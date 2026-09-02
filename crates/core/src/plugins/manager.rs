use super::manifest::{
    PluginFacetTransform, PluginKind, PluginManifest, PluginMenuAction, PluginMenuSlot,
    PluginSettingValue,
};
use super::package::{
    create_operation_dir, load_installed, stage_archive, sync_directory, InstalledPackage,
};
use super::permissions::PluginGrant;
use super::PLUGIN_API_VERSION;
use super::WASM_API_VERSION;
use super::wasm::{WasmHandle, WasmPluginSpec};
use crate::db::plugins::PluginPendingRow;
use crate::db::queries::{FacetPlan, FacetTransform};
use crate::model::BrowseMode;
use crate::Db;
use anyhow::{anyhow, bail, Context, Result};
use semver::Version;
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard};

const STARTUP_MARKER: &str = "startup-in-progress";
/// Written by the Wasm runtime when it auto-disables a plugin, and read (then
/// consumed) by the *next* start. It is deliberately not the startup marker:
/// the runtime spawns before `start` removes that one, so a fast failure could
/// re-arm it only to have this run delete it again.
const SAFE_MODE_MARKER: &str = "safe-mode-requested";
pub const GROUPING_PLUGIN_ID: &str = "dev.crescendo.primary-grouping";
pub const GROUPING_PLUGIN_SETTING: &str = "collapse_multi_tagged_titles";
const GROUPING_MIGRATION_KEY: &str = "grouping_plugin_migration";

#[derive(Debug, Clone, Serialize)]
pub struct PluginSnapshot {
    pub plugins: Vec<PluginInfo>,
    pub loaded_plugin_ids: Vec<String>,
    pub safe_mode: bool,
    pub safe_mode_reason: Option<String>,
    pub errors: Vec<String>,
    pub restart_required: bool,
    pub projection_revision: u64,
    pub menu_controls: Vec<PluginMenuControlInfo>,
    pub grouping_migration_pending: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    pub version: Option<String>,
    pub description: String,
    pub authors: Vec<String>,
    pub license: Option<String>,
    pub homepage: Option<String>,
    pub kind: Option<PluginKind>,
    pub api_version: Option<String>,
    pub min_crescendo_version: Option<String>,
    pub package_hash: Option<String>,
    pub desired_enabled: bool,
    pub loaded: bool,
    pub compatible: bool,
    pub compatibility_error: Option<String>,
    pub error: Option<String>,
    pub pending: Option<PluginPendingStatus>,
    pub settings: Vec<PluginSettingValue>,
    /// Capabilities the package requests (Wasm packages only, always empty
    /// for declarative ones).
    pub permissions: Vec<String>,
    /// Domains a `network.http` package may contact.
    pub network_domains: Vec<String>,
    /// True when no permission is requested, or the stored user grant covers
    /// exactly this package (identity, version, hash, permission set).
    pub grants_approved: bool,
    /// Command ids the package declares for `plugin_invoke`.
    pub commands: Vec<String>,
    /// Live runtime status of the sandboxed component (startup failure,
    /// traps, unhealthy session), when the runtime reports one.
    pub runtime_error: Option<String>,
    /// Tail of this session's bounded per-plugin diagnostic log.
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PluginPendingStatus {
    pub operation: String,
    pub target_version: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PluginMutationResult {
    pub changed: bool,
    pub message: Option<String>,
    pub snapshot: PluginSnapshot,
}

#[derive(Debug, Clone, Serialize)]
pub struct PluginMenuControlInfo {
    pub plugin_id: String,
    pub id: String,
    pub slot: String,
    pub label: String,
    pub target_tag: String,
    /// `"toggle_setting"` or `"run_command"`. The frontend renders a tick for
    /// the first and a plain item for the second.
    pub action: String,
    /// Set for a toggle action only.
    pub setting_key: Option<String>,
    /// Set for a command action only.
    pub command: Option<String>,
    /// The tick state of a toggle action; `None` for a command action, which
    /// has no state to show.
    pub checked: Option<bool>,
}

/// A bounded, host-resolved passive asset. Package paths never cross the
/// plugin boundary and this type is intentionally not serializable for IPC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginAsset {
    pub id: String,
    pub media_type: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
struct RegisteredFacetTransform {
    plugin_id: String,
    contribution: PluginFacetTransform,
}

#[derive(Debug, Clone, Default)]
struct ContributionRegistry {
    facets: BTreeMap<String, RegisteredFacetTransform>,
    menus: Vec<(String, super::manifest::PluginMenuControl)>,
}

#[derive(Debug, Clone)]
pub struct ProjectionSnapshot {
    pub revision: u64,
    settings: BTreeMap<(String, String), Value>,
    pub prefer_romanized: bool,
}

pub struct ProjectionReadGuard<'a> {
    snapshot: RwLockReadGuard<'a, ProjectionSnapshot>,
    registry: &'a ContributionRegistry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaleProjection {
    pub current_revision: u64,
}

impl std::fmt::Display for StaleProjection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "stale projection; current revision is {}",
            self.current_revision
        )
    }
}

impl std::error::Error for StaleProjection {}

impl ProjectionReadGuard<'_> {
    pub fn revision(&self) -> u64 {
        self.snapshot.revision
    }

    pub fn prefer_romanized(&self) -> bool {
        self.snapshot.prefer_romanized
    }

    pub fn facet_plan(&self, mode: &BrowseMode) -> Option<FacetPlan> {
        let target = browse_target_tag(mode);
        let transform = target
            .as_deref()
            .and_then(|target| self.registry.facets.get(target))
            .and_then(|registered| {
                let enabled = self
                    .snapshot
                    .settings
                    .get(&(
                        registered.plugin_id.clone(),
                        registered.contribution.enabled_setting().to_string(),
                    ))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                enabled.then(|| match &registered.contribution {
                    PluginFacetTransform::TextFirstSegment {
                        delimiter,
                        trim_whitespace,
                        ..
                    } => FacetTransform::TextFirstSegment {
                        delimiter: delimiter.clone(),
                        trim_whitespace: *trim_whitespace,
                    },
                })
            });
        FacetPlan::resolve(mode, transform.as_ref(), self.snapshot.prefer_romanized)
    }
}

pub struct PluginManager {
    installed_root: PathBuf,
    staging_root: PathBuf,
    db: Arc<Db>,
    app_version: Version,
    loaded: BTreeMap<String, PluginManifest>,
    registry: ContributionRegistry,
    projection: RwLock<ProjectionSnapshot>,
    available: bool,
    safe_mode: bool,
    safe_mode_reason: Option<String>,
    startup_errors: Vec<String>,
    operation_lock: Mutex<()>,
    wasm: Option<WasmHandle>,
    player_bridge: Arc<super::bridge::PlayerBridgeSlot>,
    host_notifier: Arc<super::bridge::HostNotifierSlot>,
}

#[derive(Debug)]
struct InstalledRecord {
    id: String,
    package: Option<InstalledPackage>,
    validation_error: Option<String>,
}

#[derive(Debug)]
struct AppliedOperation {
    row: PluginPendingRow,
    operation_dir: PathBuf,
    old_dir: PathBuf,
}

impl PluginManager {
    pub fn start(root: PathBuf, db: Arc<Db>, app_version: &str) -> Result<Self> {
        let app_version = Version::parse(app_version).context("invalid Crescendo version")?;
        let installed_root = root.join("installed");
        let staging_root = root.join("staging");
        fs::create_dir_all(&installed_root)?;
        fs::create_dir_all(&staging_root)?;

        let marker = root.join(STARTUP_MARKER);
        let incomplete = marker.try_exists()?;
        write_startup_marker(&marker)?;
        // Consume any request left by the previous run *before* the runtime
        // that writes it can start again, so a request made during this
        // startup survives into the next one.
        let safe_mode_marker = root.join(SAFE_MODE_MARKER);
        let requested = safe_mode_marker.try_exists()?;
        if requested {
            fs::remove_file(&safe_mode_marker)?;
        }
        let env_disabled =
            std::env::var_os("CRESCENDO_DISABLE_PLUGINS").is_some_and(|value| value == "1");
        let safe_mode = incomplete || env_disabled || requested;
        let safe_mode_reason = if env_disabled {
            Some("Plugins were disabled by CRESCENDO_DISABLE_PLUGINS=1.".into())
        } else if incomplete {
            Some(
                "The previous plugin startup did not finish; plugins are disabled for this run."
                    .into(),
            )
        } else if requested {
            Some(
                "A plugin failed to start repeatedly; plugins are disabled for this run.".into(),
            )
        } else {
            None
        };

        let mut startup_errors = Vec::new();
        // Filled by the shell once its player thread exists; player.* calls
        // before that answer Unavailable.
        let player_bridge = Arc::new(super::bridge::PlayerBridgeSlot::new());
        // Announces plugin-initiated changes back to the shell, so a playlist a
        // plugin writes refreshes the UI like any other write.
        let host_notifier = Arc::new(super::bridge::HostNotifierSlot::new());
        let pending = db.plugin_pending_all()?;
        let mut applied = Vec::new();
        for row in pending {
            match apply_pending_operation(&installed_root, &staging_root, &row) {
                Ok(operation) => applied.push(operation),
                Err(error) => {
                    let message = format!(
                        "Could not apply pending {} for {}: {error:#}",
                        row.operation, row.plugin_id
                    );
                    startup_errors.push(message.clone());
                    if let Ok(operation_dir) = operation_dir(&staging_root, &row.staged_path) {
                        let partial = AppliedOperation {
                            row: row.clone(),
                            old_dir: operation_dir.join("old"),
                            operation_dir: operation_dir.clone(),
                        };
                        if let Err(rollback_error) = rollback_operation(&installed_root, &partial) {
                            let rollback_message = format!(
                                "Could not restore {} after the failed operation; its journal and backup were preserved: {rollback_error:#}",
                                row.plugin_id
                            );
                            let _ = db.plugin_record_error_preserving_state(
                                &row.plugin_id,
                                &format!("{message} {rollback_message}"),
                            );
                            return Err(anyhow!("{message} {rollback_message}"));
                        }
                        let _ = fs::remove_dir_all(operation_dir);
                    }
                    db.plugin_record_error_preserving_state(&row.plugin_id, &message)?;
                    db.plugin_pending_delete(&row.plugin_id)?;
                }
            }
        }

        let mut inventory = scan_installed(&installed_root)?;
        let invalid_ids: HashSet<String> = inventory
            .iter()
            .filter(|record| record.package.is_none())
            .map(|record| record.id.clone())
            .collect();
        let mut committed = Vec::new();
        for operation in applied {
            if operation.row.operation == "update" && invalid_ids.contains(&operation.row.plugin_id)
            {
                let message = format!(
                    "Updated package for {} failed installed validation; the previous version was restored.",
                    operation.row.plugin_id
                );
                rollback_operation(&installed_root, &operation)?;
                db.plugin_record_error_preserving_state(&operation.row.plugin_id, &message)?;
                db.plugin_pending_delete(&operation.row.plugin_id)?;
                startup_errors.push(message);
                let _ = fs::remove_dir_all(&operation.operation_dir);
            } else {
                committed.push(operation);
            }
        }
        if !committed.is_empty() || !invalid_ids.is_empty() {
            inventory = scan_installed(&installed_root)?;
        }

        for record in &inventory {
            db.plugin_state_ensure(&record.id)?;
            if let Some(error) = &record.validation_error {
                db.plugin_record_error(&record.id, Some(error))?;
            }
        }

        let mut loaded = BTreeMap::new();
        let mut reserved_targets = BTreeMap::<String, String>::new();
        for record in &inventory {
            let Some(package) = &record.package else {
                continue;
            };
            let compatibility = compatibility_error(&package.manifest, &app_version);
            if compatibility.is_some() {
                db.plugin_set_desired_enabled(&record.id, false)?;
                continue;
            }
            let state =
                db.plugin_state_get(&record.id)?
                    .unwrap_or(crate::db::plugins::PluginStateRow {
                        desired_enabled: false,
                        failure_count: 0,
                        last_error: None,
                    });
            if state.desired_enabled && !safe_mode {
                // A Wasm plugin whose requested capabilities are not covered
                // by a user grant for this exact package stays unloaded. The
                // desired state is preserved so the restart banner and the
                // approve action in Settings can resolve it, and the plugin
                // reserves no facet target while it waits.
                if package.manifest.kind == PluginKind::Wasm
                    && !grants_satisfied(&db, package)?
                {
                    continue;
                }
                let conflict = package
                    .manifest
                    .contributes
                    .facet_transforms
                    .iter()
                    .find_map(|transform| {
                        reserved_targets
                            .get(transform.target_tag())
                            .map(|owner| (transform.target_tag(), owner.clone()))
                    });
                if let Some((target, owner)) = conflict {
                    let message = format!(
                        "Could not enable {} because {owner} already owns the {target} facet transform.",
                        record.id
                    );
                    db.plugin_set_desired_enabled(&record.id, false)?;
                    db.plugin_record_error(&record.id, Some(&message))?;
                    startup_errors.push(message);
                    continue;
                }
                for transform in &package.manifest.contributes.facet_transforms {
                    reserved_targets
                        .insert(transform.target_tag().to_string(), record.id.clone());
                }
                loaded.insert(record.id.clone(), package.manifest.clone());
            }
        }

        let registry = ContributionRegistry::from_loaded(&loaded);
        let projection = projection_from_loaded(&db, &loaded, 0)?;

        // Sandboxed runtime for the loaded Wasm plugins. Spawning never
        // blocks startup on guest code: instantiation and `init` run on the
        // runtime thread, and failures surface through `runtime_info`.
        let wasm = if loaded
            .values()
            .any(|manifest| manifest.kind == PluginKind::Wasm)
        {
            let specs = loaded
                .iter()
                .filter(|(_, manifest)| manifest.kind == PluginKind::Wasm)
                .map(|(id, manifest)| WasmPluginSpec {
                    id: id.clone(),
                    module_path: installed_root.join(id).join("module.wasm"),
                    manifest: Arc::new(manifest.clone()),
                })
                .collect();
            match WasmHandle::spawn(
                specs,
                db.clone(),
                Default::default(),
                player_bridge.clone(),
                host_notifier.clone(),
                super::wasm::RuntimePaths {
                    safe_mode_marker: safe_mode_marker.clone(),
                },
            ) {
                Ok(handle) => Some(handle),
                Err(error) => {
                    startup_errors.push(format!(
                        "The Wasm plugin runtime could not start: {error:#}"
                    ));
                    None
                }
            }
        } else {
            None
        };

        for operation in committed {
            if operation.row.operation == "update" {
                db.plugin_record_error(&operation.row.plugin_id, None)?;
            }
            db.plugin_pending_commit(&operation.row.plugin_id, operation.row.remove_data)?;
            if let Err(error) = fs::remove_dir_all(&operation.operation_dir) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    startup_errors.push(format!(
                        "Could not remove completed staging operation {}: {error}",
                        operation.operation_dir.display()
                    ));
                }
            }
        }
        cleanup_orphan_staging(&staging_root, &mut startup_errors)?;
        fs::remove_file(&marker)?;
        sync_directory(&root)?;

        Ok(Self {
            installed_root,
            staging_root,
            db,
            app_version,
            loaded,
            registry,
            projection: RwLock::new(projection),
            available: true,
            safe_mode,
            safe_mode_reason,
            startup_errors,
            operation_lock: Mutex::new(()),
            wasm,
            player_bridge,
            host_notifier,
        })
    }

    pub fn disabled(root: PathBuf, db: Arc<Db>, app_version: &str, error: String) -> Self {
        let app_version = Version::parse(app_version).unwrap_or_else(|_| Version::new(0, 0, 0));
        let prefer_romanized = db.prefer_romanized();
        Self {
            installed_root: root.join("installed"),
            staging_root: root.join("staging"),
            db,
            app_version,
            loaded: BTreeMap::new(),
            registry: ContributionRegistry::default(),
            projection: RwLock::new(ProjectionSnapshot {
                revision: 0,
                settings: BTreeMap::new(),
                prefer_romanized,
            }),
            available: false,
            safe_mode: true,
            safe_mode_reason: Some("The plugin system could not initialize.".into()),
            startup_errors: vec![error],
            operation_lock: Mutex::new(()),
            wasm: None,
            player_bridge: Arc::new(super::bridge::PlayerBridgeSlot::new()),
            host_notifier: Arc::new(super::bridge::HostNotifierSlot::new()),
        }
    }

    /// Install the shell's player so `player.*` host calls can work. Called
    /// once, after the player thread spawns; until then those calls answer
    /// `Unavailable` rather than blocking.
    pub fn install_player_bridge(&self, bridge: Arc<dyn super::bridge::PlayerBridge>) {
        self.player_bridge.install(bridge);
    }

    /// Install the shell's change announcer, so a playlist written by a plugin
    /// refreshes the app and reaches `on-playlist-changed` subscribers.
    pub fn install_host_notifier(&self, notifier: Arc<dyn super::bridge::HostNotifier>) {
        self.host_notifier.install(notifier);
    }

    /// Broadcast a host event to every ready Wasm plugin. Safe to call from
    /// any thread and cheap when no Wasm plugin is loaded.
    pub fn notify_event(&self, event: super::wasm::HostEvent) {
        if let Some(handle) = self.wasm.as_ref() {
            handle.notify_event(event);
        }
    }

    pub fn snapshot(&self) -> Result<PluginSnapshot> {
        let _guard = self
            .operation_lock
            .lock()
            .map_err(|_| anyhow!("plugin manager lock poisoned"))?;
        self.snapshot_unlocked()
    }

    /// Capture one immutable projection generation. The returned guard keeps
    /// publication blocked until its complete query has finished.
    pub fn projection(&self, expected_revision: Option<u64>) -> Result<ProjectionReadGuard<'_>> {
        let snapshot = self
            .projection
            .read()
            .map_err(|_| anyhow!("plugin projection lock poisoned"))?;
        if let Some(expected) = expected_revision {
            if expected != snapshot.revision {
                return Err(StaleProjection {
                    current_revision: snapshot.revision,
                }
                .into());
            }
        }
        Ok(ProjectionReadGuard {
            snapshot,
            registry: &self.registry,
        })
    }

    /// Resolve a passive asset belonging to a plugin in the immutable loaded
    /// set. Callers name an asset id relative to `assets/`; they never receive
    /// or supply an installed-package path.
    pub fn load_asset(&self, plugin_id: &str, asset_id: &str) -> Result<PluginAsset> {
        self.ensure_available()?;
        if !self.loaded.contains_key(plugin_id) || !is_safe_plugin_component(plugin_id) {
            bail!("plugin {plugin_id} is not loaded");
        }
        let relative = normalize_asset_id(asset_id)?;
        let package_root = self.installed_root.join(plugin_id);
        // Revalidate the installed package before opening an asset so an
        // external mutation cannot turn a passive loaded package into code.
        load_installed(&package_root, plugin_id)?;
        let assets_root = fs::canonicalize(package_root.join("assets"))
            .context("plugin has no readable assets directory")?;
        let candidate = package_root.join("assets").join(&relative);
        let candidate_meta = fs::symlink_metadata(&candidate)
            .with_context(|| format!("plugin asset {asset_id:?} does not exist"))?;
        if !candidate_meta.file_type().is_file() || candidate_meta.file_type().is_symlink() {
            bail!("plugin asset is not a regular file");
        }
        let resolved = fs::canonicalize(&candidate)?;
        if !resolved.starts_with(&assets_root) {
            bail!("plugin asset escapes its package");
        }
        if candidate_meta.len() > super::package::MAX_EXTRACTED_BYTES {
            bail!("plugin asset exceeds the 64 MiB read limit");
        }
        let mut reader = File::open(&resolved)?.take(super::package::MAX_EXTRACTED_BYTES + 1);
        let mut bytes = Vec::with_capacity(candidate_meta.len() as usize);
        reader.read_to_end(&mut bytes)?;
        if bytes.len() as u64 > super::package::MAX_EXTRACTED_BYTES {
            bail!("plugin asset exceeds the 64 MiB read limit");
        }
        Ok(PluginAsset {
            id: asset_id.to_string(),
            media_type: asset_media_type(&relative)?.to_string(),
            bytes,
        })
    }

    /// Persist and publish the host romanization preference through the same
    /// generation boundary as plugin projection settings.
    pub fn set_prefer_romanized(&self, value: bool) -> Result<u64> {
        let _guard = self
            .operation_lock
            .lock()
            .map_err(|_| anyhow!("plugin manager lock poisoned"))?;
        self.db
            .prefs_set("prefer_romanized", if value { "true" } else { "false" })?;
        let mut projection = self
            .projection
            .write()
            .map_err(|_| anyhow!("plugin projection lock poisoned"))?;
        if projection.prefer_romanized != value {
            projection.prefer_romanized = value;
            projection.revision = projection.revision.saturating_add(1);
        }
        Ok(projection.revision)
    }

    pub fn complete_grouping_migration(
        &self,
        bundled_archive: Option<&Path>,
    ) -> Result<PluginMutationResult> {
        if self.db.meta_get(GROUPING_MIGRATION_KEY)?.as_deref() != Some("pending") {
            return Ok(PluginMutationResult {
                changed: false,
                message: None,
                snapshot: self.snapshot()?,
            });
        }

        if let Some(archive) = bundled_archive {
            if !self.installed_root.join(GROUPING_PLUGIN_ID).try_exists()? {
                self.install(archive)?;
            }
            self.set_setting(
                GROUPING_PLUGIN_ID,
                GROUPING_PLUGIN_SETTING,
                Value::Bool(true),
            )?;
            self.set_enabled(GROUPING_PLUGIN_ID, true)?;
        }
        self.db.meta_set(GROUPING_MIGRATION_KEY, "completed")?;
        self.db.prefs_delete("grouping_split_multi")?;
        Ok(PluginMutationResult {
            changed: true,
            message: Some(if bundled_archive.is_some() {
                "The grouping plugin will be enabled after Crescendo restarts.".into()
            } else {
                "The grouping plugin notice was dismissed.".into()
            }),
            snapshot: self.snapshot()?,
        })
    }

    pub fn install(&self, archive_path: &Path) -> Result<PluginMutationResult> {
        self.ensure_available()?;
        let staged = stage_archive(archive_path, &self.staging_root)?;
        let _guard = self
            .operation_lock
            .lock()
            .map_err(|_| anyhow!("plugin manager lock poisoned"))?;
        let mut journalled = false;
        let result = (|| {
            let plugin_id = staged.manifest.id.clone();
            if self
                .db
                .plugin_pending_all()?
                .iter()
                .any(|pending| pending.plugin_id == plugin_id)
            {
                bail!("{plugin_id} already has a pending package operation");
            }
            let installed_path = self.installed_root.join(&plugin_id);
            if installed_path.try_exists()? {
                let current = load_installed(&installed_path, &plugin_id)
                    .context("the installed plugin must be valid before it can be updated")?;
                let current_version = Version::parse(&current.manifest.version)?;
                let next_version = Version::parse(&staged.manifest.version)?;
                if next_version <= current_version {
                    bail!(
                        "updates must have a higher version than the installed {}",
                        current.manifest.version
                    );
                }
                if self
                    .db
                    .plugin_state_get(&plugin_id)?
                    .is_some_and(|state| state.desired_enabled)
                {
                    self.ensure_manifest_has_no_conflict(&plugin_id, &staged.manifest)?;
                }
                let pending = PluginPendingRow {
                    plugin_id: plugin_id.clone(),
                    operation: "update".into(),
                    staged_path: staged.operation_id.clone(),
                    target_version: Some(staged.manifest.version.clone()),
                    package_hash: Some(staged.package_hash.clone()),
                    remove_data: false,
                };
                self.db.plugin_schedule_pending(&pending, None)?;
                // From this point the staging directory belongs to the
                // durable journal, even if building the response snapshot
                // encounters a later database or filesystem error.
                journalled = true;
                Ok(PluginMutationResult {
                    changed: true,
                    message: Some(format!(
                        "{} will update after Crescendo restarts.",
                        staged.manifest.name
                    )),
                    snapshot: self.snapshot_unlocked()?,
                })
            } else {
                fs::rename(&staged.content_dir, &installed_path)
                    .context("could not promote the installed plugin")?;
                let finish = sync_directory(&self.installed_root).and_then(|_| {
                    self.db
                        .plugin_set_desired_enabled(&plugin_id, false)
                        .and_then(|_| self.db.plugin_record_error(&plugin_id, None))
                });
                if let Err(error) = finish {
                    let rollback = fs::rename(&installed_path, &staged.content_dir)
                        .context("could not move the promoted plugin back into staging")
                        .and_then(|_| sync_directory(&self.installed_root));
                    if let Err(rollback_error) = rollback {
                        return Err(error).context(format!(
                            "the install also could not be rolled back: {rollback_error:#}"
                        ));
                    }
                    return Err(error);
                }
                let _ = fs::remove_dir_all(&staged.operation_dir);
                Ok(PluginMutationResult {
                    changed: true,
                    message: Some(format!("{} was installed disabled.", staged.manifest.name)),
                    snapshot: self.snapshot_unlocked()?,
                })
            }
        })();
        if result.is_err() && !journalled {
            let _ = fs::remove_dir_all(&staged.operation_dir);
        }
        result
    }

    pub fn set_enabled(&self, plugin_id: &str, enabled: bool) -> Result<PluginMutationResult> {
        self.ensure_available()?;
        let _guard = self
            .operation_lock
            .lock()
            .map_err(|_| anyhow!("plugin manager lock poisoned"))?;
        let record = self.valid_installed(plugin_id)?;
        if self
            .db
            .plugin_pending_all()?
            .iter()
            .any(|pending| pending.plugin_id == plugin_id && pending.operation == "uninstall")
        {
            bail!("the plugin is pending uninstall");
        }
        if enabled {
            if let Some(error) = compatibility_error(&record.manifest, &self.app_version) {
                bail!("{error}");
            }
            if record.manifest.kind == PluginKind::Wasm && !grants_satisfied(&self.db, &record)? {
                bail!(
                    "{} requests capabilities that require approval before it can be enabled",
                    record.manifest.name
                );
            }
            self.ensure_manifest_has_no_conflict(plugin_id, &record.manifest)?;
        }
        let old = self
            .db
            .plugin_state_get(plugin_id)?
            .map(|state| state.desired_enabled)
            .unwrap_or(false);
        let loaded = self.loaded.contains_key(plugin_id);
        self.db.plugin_set_desired_enabled(plugin_id, enabled)?;
        Ok(PluginMutationResult {
            changed: old != enabled,
            message: (old != enabled).then(|| {
                if enabled == loaded {
                    format!(
                        "The pending change was cancelled; {} remains {}.",
                        record.manifest.name,
                        if enabled { "enabled" } else { "disabled" }
                    )
                } else {
                    format!(
                        "{} will be {} after Crescendo restarts.",
                        record.manifest.name,
                        if enabled { "enabled" } else { "disabled" }
                    )
                }
            }),
            snapshot: self.snapshot_unlocked()?,
        })
    }

    pub fn set_setting(
        &self,
        plugin_id: &str,
        key: &str,
        value: Value,
    ) -> Result<PluginMutationResult> {
        self.ensure_available()?;
        let _guard = self
            .operation_lock
            .lock()
            .map_err(|_| anyhow!("plugin manager lock poisoned"))?;
        let record = self.valid_installed(plugin_id)?;
        let declaration = record
            .manifest
            .contributes
            .settings
            .iter()
            .find(|setting| setting.key() == key)
            .ok_or_else(|| anyhow!("plugin does not declare setting {key:?}"))?;
        declaration.validate_value(&value)?;
        let previous = self.db.plugin_settings_get(plugin_id)?.get(key).cloned();
        self.db.plugin_setting_set(plugin_id, key, &value)?;
        if self.loaded.contains_key(plugin_id) {
            let affects_projection = self.registry.setting_affects_projection(plugin_id, key);
            let mut projection = self
                .projection
                .write()
                .map_err(|_| anyhow!("plugin projection lock poisoned"))?;
            projection
                .settings
                .insert((plugin_id.to_string(), key.to_string()), value.clone());
            if affects_projection && previous.as_ref() != Some(&value) {
                projection.revision = projection.revision.saturating_add(1);
            }
            // Sandbox components read live values through the on-demand
            // settings API; the coalesced notification tells them to look.
            if let Some(handle) = &self.wasm {
                handle.notify_settings_changed(plugin_id);
            }
        }
        Ok(PluginMutationResult {
            changed: previous.as_ref() != Some(&value),
            message: None,
            snapshot: self.snapshot_unlocked()?,
        })
    }

    pub fn uninstall(&self, plugin_id: &str, remove_data: bool) -> Result<PluginMutationResult> {
        self.ensure_available()?;
        let _guard = self
            .operation_lock
            .lock()
            .map_err(|_| anyhow!("plugin manager lock poisoned"))?;
        if !is_safe_plugin_component(plugin_id) {
            bail!("invalid plugin id");
        }
        let installed_path = self.installed_root.join(plugin_id);
        if !installed_path.try_exists()? {
            bail!("plugin {plugin_id} is not installed");
        }
        let display_name = load_installed(&installed_path, plugin_id)
            .map(|package| package.manifest.name)
            .unwrap_or_else(|_| plugin_id.to_string());

        if let Some(existing) = self
            .db
            .plugin_pending_all()?
            .into_iter()
            .find(|pending| pending.plugin_id == plugin_id)
        {
            if existing.operation != "update" {
                bail!("the plugin already has a pending operation");
            }
            let operation_dir = operation_dir(&self.staging_root, &existing.staged_path)?;
            self.db.plugin_pending_delete(plugin_id)?;
            let _ = fs::remove_dir_all(operation_dir);
        }

        let (operation_id, operation_path) = create_operation_dir(&self.staging_root)?;
        let pending = PluginPendingRow {
            plugin_id: plugin_id.into(),
            operation: "uninstall".into(),
            staged_path: operation_id,
            target_version: None,
            package_hash: None,
            remove_data,
        };
        if let Err(error) = self.db.plugin_schedule_pending(&pending, Some(false)) {
            let _ = fs::remove_dir_all(&operation_path);
            return Err(error);
        }

        if self.loaded.contains_key(plugin_id) {
            return Ok(PluginMutationResult {
                changed: true,
                message: Some(format!(
                    "{display_name} will be uninstalled after Crescendo restarts."
                )),
                snapshot: self.snapshot_unlocked()?,
            });
        }

        let applied =
            match apply_pending_operation(&self.installed_root, &self.staging_root, &pending) {
                Ok(applied) => applied,
                Err(error) => {
                    self.db.plugin_pending_delete(plugin_id)?;
                    let _ = fs::remove_dir_all(&operation_path);
                    return Err(error);
                }
            };
        self.db.plugin_pending_commit(plugin_id, remove_data)?;
        if let Err(error) = fs::remove_dir_all(&applied.operation_dir) {
            if error.kind() != std::io::ErrorKind::NotFound {
                log::warn!(
                    "could not remove completed plugin operation {}: {error}",
                    applied.operation_dir.display()
                );
            }
        }
        sync_directory(&self.installed_root)?;
        Ok(PluginMutationResult {
            changed: true,
            message: Some(format!("{display_name} was uninstalled.")),
            snapshot: self.snapshot_unlocked()?,
        })
    }

    /// Record the user's approval of the capabilities requested by the
    /// currently installed Wasm package. The grant is pinned to the exact
    /// package version and hash, so any later update voids it and must be
    /// approved again.
    pub fn approve_grants(&self, plugin_id: &str) -> Result<PluginMutationResult> {
        self.ensure_available()?;
        let _guard = self
            .operation_lock
            .lock()
            .map_err(|_| anyhow!("plugin manager lock poisoned"))?;
        let record = self.valid_installed(plugin_id)?;
        if record.manifest.kind != PluginKind::Wasm {
            bail!("only Wasm plugins request capabilities");
        }
        let permissions = super::permissions::parse_permissions(&record.manifest.permissions)?;
        if permissions.is_empty() {
            bail!("the plugin requests no capabilities");
        }
        if let Some(error) = compatibility_error(&record.manifest, &self.app_version) {
            bail!("{error}");
        }
        let grant = PluginGrant {
            plugin_id: plugin_id.to_string(),
            permissions,
            network_domains: record.manifest.network_domains.clone(),
            version: record.manifest.version.clone(),
            package_hash: record.package_hash.clone(),
            granted_at: super::wasm::now_unix(),
        };
        self.db.plugin_grant_set(&grant)?;
        Ok(PluginMutationResult {
            changed: true,
            message: Some(format!(
                "Capabilities approved for {} {}. Enable the plugin and restart to apply.",
                record.manifest.name, record.manifest.version
            )),
            snapshot: self.snapshot_unlocked()?,
        })
    }

    /// Dispatch one host-visible, registered command of a loaded Wasm plugin.
    /// Input and output are bounded JSON documents; track IDs and other bulk
    /// data never ride along implicitly.
    pub fn invoke(&self, plugin_id: &str, command_id: &str, input: &Value) -> Result<Value> {
        self.ensure_available()?;
        let manifest = self
            .loaded
            .get(plugin_id)
            .ok_or_else(|| anyhow!("plugin {plugin_id} is not loaded"))?;
        if manifest.kind != PluginKind::Wasm {
            bail!("plugin {plugin_id} has no Wasm commands");
        }
        if !manifest
            .contributes
            .commands
            .iter()
            .any(|command| command.id == command_id)
        {
            bail!("plugin {plugin_id} does not declare command {command_id:?}");
        }
        let payload = serde_json::to_string(input)?;
        if payload.len() > super::wasm::MAX_INVOKE_INPUT_BYTES {
            bail!("plugin command input exceeds 1 MiB");
        }
        let handle = self
            .wasm
            .as_ref()
            .ok_or_else(|| anyhow!("the Wasm plugin runtime is not running"))?;
        let output = handle.invoke(plugin_id, command_id, &payload)?;
        if output.len() > super::wasm::MAX_INVOKE_OUTPUT_BYTES {
            bail!("plugin command output exceeds 1 MiB");
        }
        serde_json::from_str(&output).context("plugin command output is not valid JSON")
    }

    fn snapshot_unlocked(&self) -> Result<PluginSnapshot> {
        let inventory = scan_installed(&self.installed_root).unwrap_or_else(|error| {
            vec![InstalledRecord {
                id: "plugin-system".into(),
                package: None,
                validation_error: Some(error.to_string()),
            }]
        });
        let pending: HashMap<String, PluginPendingRow> = self
            .db
            .plugin_pending_all()?
            .into_iter()
            .map(|row| (row.plugin_id.clone(), row))
            .collect();
        let mut plugins = Vec::new();
        let mut errors = self.startup_errors.clone();
        let mut restart_required = !pending.is_empty();

        for record in inventory {
            let state = self.db.plugin_state_get(&record.id)?;
            let desired_enabled = state.as_ref().is_some_and(|state| state.desired_enabled);
            let loaded = self.loaded.contains_key(&record.id);
            restart_required |= desired_enabled != loaded;
            let pending_status = pending.get(&record.id).map(|row| PluginPendingStatus {
                operation: row.operation.clone(),
                target_version: row.target_version.clone(),
            });
            let state_error = state.and_then(|state| state.last_error);

            if let Some(package) = record.package {
                let compatibility_error = compatibility_error(&package.manifest, &self.app_version);
                let compatible = compatibility_error.is_none();
                let stored = self.db.plugin_settings_get(&record.id)?;
                let settings = package
                    .manifest
                    .contributes
                    .settings
                    .iter()
                    .cloned()
                    .map(|declaration| {
                        let value = stored
                            .get(declaration.key())
                            .filter(|value| declaration.validate_value(value).is_ok())
                            .cloned()
                            .unwrap_or_else(|| declaration.default_value());
                        let affects_projection = self
                            .registry
                            .setting_affects_projection(&record.id, declaration.key());
                        PluginSettingValue {
                            declaration,
                            value,
                            affects_projection,
                        }
                    })
                    .collect();
                plugins.push(PluginInfo {
                    id: record.id.clone(),
                    name: package.manifest.name.clone(),
                    version: Some(package.manifest.version.clone()),
                    description: package.manifest.description.clone(),
                    authors: package.manifest.authors.clone(),
                    license: Some(package.manifest.license.clone()),
                    homepage: package.manifest.homepage.clone(),
                    kind: Some(package.manifest.kind),
                    api_version: Some(package.manifest.api_version.clone()),
                    min_crescendo_version: Some(package.manifest.min_crescendo_version.clone()),
                    package_hash: Some(package.package_hash.clone()),
                    desired_enabled,
                    loaded,
                    compatible,
                    compatibility_error,
                    error: state_error,
                    pending: pending_status,
                    settings,
                    permissions: package.manifest.permissions.clone(),
                    network_domains: package.manifest.network_domains.clone(),
                    grants_approved: grants_satisfied(&self.db, &package)?,
                    commands: package
                        .manifest
                        .contributes
                        .commands
                        .iter()
                        .map(|command| command.id.clone())
                        .collect(),
                    runtime_error: self
                        .wasm
                        .as_ref()
                        .and_then(|handle| {
                            handle.runtime_info(&record.id).error
                        }),
                    diagnostics: self
                        .wasm
                        .as_ref()
                        .map(|handle| handle.runtime_info(&record.id).diagnostics)
                        .unwrap_or_default(),
                });
            } else {
                let validation_error = record.validation_error.or(state_error);
                if let Some(error) = &validation_error {
                    errors.push(format!("{}: {error}", record.id));
                }
                plugins.push(PluginInfo {
                    id: record.id.clone(),
                    name: record.id,
                    version: None,
                    description: String::new(),
                    authors: vec![],
                    license: None,
                    homepage: None,
                    kind: None,
                    api_version: None,
                    min_crescendo_version: None,
                    package_hash: None,
                    desired_enabled,
                    loaded,
                    compatible: false,
                    compatibility_error: Some("The installed package is invalid.".into()),
                    error: validation_error,
                    pending: pending_status,
                    settings: vec![],
                    permissions: vec![],
                    network_domains: vec![],
                    grants_approved: true,
                    commands: vec![],
                    runtime_error: None,
                    diagnostics: vec![],
                });
            }
        }
        plugins.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then(a.id.cmp(&b.id))
        });
        Ok(PluginSnapshot {
            plugins,
            loaded_plugin_ids: self.loaded.keys().cloned().collect(),
            safe_mode: self.safe_mode,
            safe_mode_reason: self.safe_mode_reason.clone(),
            errors,
            restart_required,
            projection_revision: self
                .projection
                .read()
                .map_err(|_| anyhow!("plugin projection lock poisoned"))?
                .revision,
            menu_controls: self.menu_controls()?,
            grouping_migration_pending: self
                .db
                .meta_get(GROUPING_MIGRATION_KEY)?
                .as_deref()
                == Some("pending"),
        })
    }

    fn valid_installed(&self, plugin_id: &str) -> Result<InstalledPackage> {
        if !is_safe_plugin_component(plugin_id) {
            bail!("invalid plugin id");
        }
        let path = self.installed_root.join(plugin_id);
        if !path.try_exists()? {
            bail!("plugin {plugin_id} is not installed");
        }
        load_installed(&path, plugin_id)
    }

    fn ensure_available(&self) -> Result<()> {
        if !self.available {
            bail!("the plugin system is unavailable; restart Crescendo after resolving the startup error");
        }
        Ok(())
    }

    fn ensure_manifest_has_no_conflict(
        &self,
        plugin_id: &str,
        manifest: &PluginManifest,
    ) -> Result<()> {
        let targets: HashSet<&str> = manifest
            .contributes
            .facet_transforms
            .iter()
            .map(PluginFacetTransform::target_tag)
            .collect();
        if targets.is_empty() {
            return Ok(());
        }
        for record in scan_installed(&self.installed_root)? {
            if record.id == plugin_id
                || !self
                    .db
                    .plugin_state_get(&record.id)?
                    .is_some_and(|state| state.desired_enabled)
            {
                continue;
            }
            let Some(package) = record.package else {
                continue;
            };
            if let Some(target) = package
                .manifest
                .contributes
                .facet_transforms
                .iter()
                .map(PluginFacetTransform::target_tag)
                .find(|target| targets.contains(target))
            {
                bail!(
                    "{} already owns the {target} facet transform; disable it before enabling {plugin_id}",
                    record.id
                );
            }
        }
        Ok(())
    }

    fn menu_controls(&self) -> Result<Vec<PluginMenuControlInfo>> {
        let projection = self
            .projection
            .read()
            .map_err(|_| anyhow!("plugin projection lock poisoned"))?;
        Ok(self
            .registry
            .menus
            .iter()
            .map(|(plugin_id, control)| PluginMenuControlInfo {
                plugin_id: plugin_id.clone(),
                id: control.id.clone(),
                slot: match control.slot {
                    PluginMenuSlot::LibraryFilter => "library.filter".into(),
                },
                label: control.label.clone(),
                target_tag: control.visible_when.active_tag.clone(),
                action: match &control.action {
                    PluginMenuAction::ToggleSetting { .. } => "toggle_setting".into(),
                    PluginMenuAction::RunCommand { .. } => "run_command".into(),
                },
                setting_key: control.action.setting().map(str::to_string),
                command: control.action.command().map(str::to_string),
                checked: control.checked_setting.as_ref().map(|setting| {
                    projection
                        .settings
                        .get(&(plugin_id.clone(), setting.clone()))
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                }),
            })
            .collect())
    }
}

impl ContributionRegistry {
    fn from_loaded(loaded: &BTreeMap<String, PluginManifest>) -> Self {
        let mut registry = Self::default();
        for (plugin_id, manifest) in loaded {
            for contribution in &manifest.contributes.facet_transforms {
                registry.facets.insert(
                    contribution.target_tag().to_string(),
                    RegisteredFacetTransform {
                        plugin_id: plugin_id.clone(),
                        contribution: contribution.clone(),
                    },
                );
            }
            registry.menus.extend(
                manifest
                    .contributes
                    .menu_controls
                    .iter()
                    .cloned()
                    .map(|control| (plugin_id.clone(), control)),
            );
        }
        registry
    }

    fn setting_affects_projection(&self, plugin_id: &str, key: &str) -> bool {
        self.facets.values().any(|registered| {
            registered.plugin_id == plugin_id && registered.contribution.enabled_setting() == key
        })
    }
}

fn projection_from_loaded(
    db: &Db,
    loaded: &BTreeMap<String, PluginManifest>,
    revision: u64,
) -> Result<ProjectionSnapshot> {
    let mut settings = BTreeMap::new();
    for (plugin_id, manifest) in loaded {
        let stored = db.plugin_settings_get(plugin_id)?;
        for declaration in &manifest.contributes.settings {
            let value = stored
                .get(declaration.key())
                .filter(|value| declaration.validate_value(value).is_ok())
                .cloned()
                .unwrap_or_else(|| declaration.default_value());
            settings.insert((plugin_id.clone(), declaration.key().to_string()), value);
        }
    }
    Ok(ProjectionSnapshot {
        revision,
        settings,
        prefer_romanized: db.prefer_romanized(),
    })
}

fn browse_target_tag(mode: &BrowseMode) -> Option<String> {
    match mode {
        BrowseMode::Genre => Some("genre".into()),
        BrowseMode::Artist => Some("artist".into()),
        BrowseMode::Album => Some("album".into()),
        BrowseMode::Albumartist => Some("albumartist".into()),
        BrowseMode::Custom(tag) if mode.column().is_some() => Some(tag.trim().to_ascii_lowercase()),
        BrowseMode::Custom(_) => None,
    }
}

fn compatibility_error(manifest: &PluginManifest, app_version: &Version) -> Option<String> {
    let (supported, api_label) = match manifest.kind {
        PluginKind::Declarative => (
            Version::parse(PLUGIN_API_VERSION).expect("valid host plugin API version"),
            "plugin API",
        ),
        PluginKind::Wasm => {
            // A Wasm package may also name capabilities this build's host
            // does not provide yet. It stays installed with an explanation.
            if let Err(error) = super::permissions::parse_permissions(&manifest.permissions) {
                return Some(error.to_string());
            }
            for permission in manifest.permissions.iter().filter_map(|name| {
                super::permissions::PluginPermission::parse(name)
            }) {
                if !permission.runtime_supported() {
                    return Some(format!(
                        "Requests the \"{}\" capability, which this build does not yet provide.",
                        permission.as_str()
                    ));
                }
            }
            (
                Version::parse(WASM_API_VERSION).expect("valid host Wasm API version"),
                "Wasm runtime API",
            )
        }
    };
    let requested = Version::parse(&manifest.api_version).ok()?;
    // Semver treats a 0.x minor bump as breaking, and the plugin world is
    // still 0.x: adding an exported event to `commands` makes an older
    // component fail to instantiate, so it must read as incompatible here
    // rather than load and trap.
    let breaking_minor = supported.major == 0 && requested.minor != supported.minor;
    if requested.major != supported.major || breaking_minor || requested > supported {
        return Some(format!(
            "Requires {api_label} {}, but this build supports {supported}.",
            manifest.api_version
        ));
    }
    let minimum = Version::parse(&manifest.min_crescendo_version).ok()?;
    if minimum > *app_version {
        return Some(format!("Requires Crescendo {minimum} or newer."));
    }
    None
}

/// Whether the stored user grant covers exactly the package's requested
/// capabilities. Plugins that request nothing need no grant.
fn grants_satisfied(db: &Db, package: &InstalledPackage) -> Result<bool> {
    let manifest = &package.manifest;
    let permissions = super::permissions::parse_permissions(&manifest.permissions)?;
    if permissions.is_empty() {
        return Ok(true);
    }
    Ok(db
        .plugin_grant_get(&manifest.id)?
        .is_some_and(|grant| {
            grant.approves(
                &manifest.id,
                &permissions,
                &manifest.network_domains,
                &manifest.version,
                &package.package_hash,
            )
        }))
}

fn scan_installed(installed_root: &Path) -> Result<Vec<InstalledRecord>> {
    let mut records = Vec::new();
    if !installed_root.try_exists()? {
        return Ok(records);
    }
    for entry in fs::read_dir(installed_root)? {
        let entry = entry?;
        let id = match entry.file_name().into_string() {
            Ok(id) => id,
            Err(_) => {
                records.push(InstalledRecord {
                    id: "<non-UTF-8 plugin directory>".into(),
                    package: None,
                    validation_error: Some("Installed plugin directory is not valid UTF-8.".into()),
                });
                continue;
            }
        };
        if !is_safe_plugin_component(&id) {
            records.push(InstalledRecord {
                id,
                package: None,
                validation_error: Some("Installed plugin directory has an invalid ID.".into()),
            });
            continue;
        }
        match load_installed(&entry.path(), &id) {
            Ok(package) => records.push(InstalledRecord {
                id,
                package: Some(package),
                validation_error: None,
            }),
            Err(error) => records.push(InstalledRecord {
                id,
                package: None,
                validation_error: Some(format!("{error:#}")),
            }),
        }
    }
    records.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(records)
}

fn apply_pending_operation(
    installed_root: &Path,
    staging_root: &Path,
    row: &PluginPendingRow,
) -> Result<AppliedOperation> {
    if !is_safe_plugin_component(&row.plugin_id) {
        bail!("pending operation has an invalid plugin id");
    }
    let operation_dir = operation_dir(staging_root, &row.staged_path)?;
    let installed = installed_root.join(&row.plugin_id);
    let new = operation_dir.join("new");
    let old = operation_dir.join("old");
    match row.operation.as_str() {
        "update" => {
            let candidate = if new.try_exists()? {
                &new
            } else if installed.try_exists()? {
                &installed
            } else {
                bail!("update staging contains no replacement package");
            };
            let candidate = load_installed(candidate, &row.plugin_id)
                .context("the staged update is no longer valid")?;
            if row.target_version.as_deref() != Some(candidate.manifest.version.as_str()) {
                bail!("the staged update version does not match its journal");
            }
            if row.package_hash.as_deref() != Some(candidate.package_hash.as_str()) {
                bail!("the staged update source hash does not match its journal");
            }
            if installed.try_exists()? && old.try_exists()? && new.try_exists()? {
                bail!("update staging contains conflicting old, new, and installed copies");
            }
            if installed.try_exists()? && !old.try_exists()? && new.try_exists()? {
                fs::rename(&installed, &old)?;
                sync_directory(installed_root)?;
                sync_directory(&operation_dir)?;
            }
            if !installed.try_exists()? && new.try_exists()? {
                fs::rename(&new, &installed)?;
                sync_directory(installed_root)?;
                sync_directory(&operation_dir)?;
            }
            if !installed.try_exists()? || !old.try_exists()? || new.try_exists()? {
                bail!("update staging is incomplete");
            }
        }
        "uninstall" => {
            if installed.try_exists()? && !old.try_exists()? {
                fs::rename(&installed, &old)?;
                sync_directory(installed_root)?;
                sync_directory(&operation_dir)?;
            }
            if installed.try_exists()? || !old.try_exists()? {
                bail!("uninstall staging is incomplete");
            }
        }
        other => bail!("unknown pending plugin operation {other:?}"),
    }
    Ok(AppliedOperation {
        row: row.clone(),
        operation_dir,
        old_dir: old,
    })
}

fn rollback_operation(installed_root: &Path, operation: &AppliedOperation) -> Result<()> {
    let installed = installed_root.join(&operation.row.plugin_id);
    if !operation.old_dir.try_exists()? {
        return Ok(());
    }
    let rejected = operation.operation_dir.join("rejected");
    if installed.try_exists()? {
        if rejected.try_exists()? {
            fs::remove_dir_all(&rejected)?;
        }
        fs::rename(&installed, &rejected)?;
    }
    fs::rename(&operation.old_dir, &installed)?;
    if rejected.try_exists()? {
        fs::remove_dir_all(rejected)?;
    }
    sync_directory(installed_root)
}

fn operation_dir(staging_root: &Path, staged_path: &str) -> Result<PathBuf> {
    if staged_path.is_empty()
        || staged_path.len() > 128
        || !staged_path.starts_with("op-")
        || !staged_path
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        bail!("pending operation contains an unsafe staging path");
    }
    let path = staging_root.join(staged_path);
    if !path.try_exists()? || !fs::symlink_metadata(&path)?.file_type().is_dir() {
        bail!("pending operation staging directory is missing");
    }
    Ok(path)
}

fn cleanup_orphan_staging(staging_root: &Path, errors: &mut Vec<String>) -> Result<()> {
    for entry in fs::read_dir(staging_root)? {
        let entry = entry?;
        let path = entry.path();
        let result = if entry.file_type()?.is_dir() {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
        if let Err(error) = result {
            errors.push(format!(
                "Could not remove orphan plugin staging path {}: {error}",
                path.display()
            ));
        }
    }
    Ok(())
}

fn write_startup_marker(path: &Path) -> Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)?;
    writeln!(
        file,
        "pid={} started={:?}",
        std::process::id(),
        std::time::SystemTime::now()
    )?;
    file.sync_all()?;
    if let Some(parent) = path.parent() {
        sync_directory(parent)?;
    }
    Ok(())
}

fn is_safe_plugin_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value != "."
        && value != ".."
        && !value.contains('/')
        && !value.contains('\\')
        && value.is_ascii()
}

fn normalize_asset_id(value: &str) -> Result<PathBuf> {
    if value.is_empty()
        || value.len() > 1024
        || value.starts_with('/')
        || value.starts_with('\\')
        || value.contains('\\')
        || value.contains('\0')
    {
        bail!("invalid plugin asset id");
    }
    let mut path = PathBuf::new();
    for segment in value.split('/') {
        if segment.is_empty()
            || segment == "."
            || segment == ".."
            || segment.contains(':')
            || segment.chars().any(char::is_control)
        {
            bail!("invalid plugin asset id");
        }
        path.push(segment);
    }
    asset_media_type(&path)?;
    Ok(path)
}

fn asset_media_type(path: &Path) -> Result<&'static str> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| anyhow!("plugin asset has no supported extension"))?;
    match extension.as_str() {
        "png" => Ok("image/png"),
        "jpg" | "jpeg" => Ok("image/jpeg"),
        "webp" => Ok("image/webp"),
        "gif" => Ok("image/gif"),
        "bmp" => Ok("image/bmp"),
        "txt" => Ok("text/plain; charset=utf-8"),
        "md" => Ok("text/markdown; charset=utf-8"),
        "json" => Ok("application/json"),
        _ => bail!("plugin asset has an unsupported or active-content extension"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::package::write_test_package;
    use crate::plugins::{PluginContributions, PluginSelectOption, PluginSettingDeclaration};
    use std::fs::File;

    fn manifest(version: &str) -> PluginManifest {
        PluginManifest {
            schema_version: 1,
            id: "dev.crescendo.manager-test".into(),
            name: "Manager Test".into(),
            version: version.into(),
            description: "Lifecycle test".into(),
            authors: vec!["Crescendo".into()],
            license: "MIT".into(),
            homepage: None,
            kind: PluginKind::Declarative,
            api_version: "1.0.0".into(),
            min_crescendo_version: "0.8.0".into(),
            permissions: vec![],
            network_domains: vec![],
            contributes: PluginContributions {
                settings: vec![PluginSettingDeclaration::Select {
                    key: "mode".into(),
                    label: "Mode".into(),
                    description: String::new(),
                    default: "one".into(),
                    options: vec![
                        PluginSelectOption {
                            value: "one".into(),
                            label: "One".into(),
                        },
                        PluginSelectOption {
                            value: "two".into(),
                            label: "Two".into(),
                        },
                    ],
                }],
                ..PluginContributions::default()
            },
        }
    }

    fn transform_manifest(id: &str) -> PluginManifest {
        let mut value = manifest("1.0.0");
        value.id = id.into();
        value.name = id.into();
        value.contributes.settings.push(PluginSettingDeclaration::Boolean {
            key: "collapse_titles".into(),
            label: "Collapse titles".into(),
            description: String::new(),
            default: true,
        });
        value
            .contributes
            .facet_transforms
            .push(PluginFacetTransform::TextFirstSegment {
                target_tag: "grouping".into(),
                delimiter: ";".into(),
                trim_whitespace: true,
                enabled_setting: "collapse_titles".into(),
            });
        value.contributes.menu_controls.push(super::super::manifest::PluginMenuControl {
            id: "collapse_titles".into(),
            slot: PluginMenuSlot::LibraryFilter,
            label: "Collapse titles".into(),
            visible_when: super::super::manifest::PluginMenuVisibility {
                active_tag: "grouping".into(),
            },
            checked_setting: Some("collapse_titles".into()),
            action: PluginMenuAction::ToggleSetting {
                setting: "collapse_titles".into(),
            },
        });
        value
    }

    fn official_grouping_manifest() -> PluginManifest {
        let mut value = transform_manifest(GROUPING_PLUGIN_ID);
        if let PluginSettingDeclaration::Boolean { key, .. } = &mut value.contributes.settings[1]
        {
            *key = GROUPING_PLUGIN_SETTING.into();
        }
        let PluginFacetTransform::TextFirstSegment {
            enabled_setting, ..
        } = &mut value.contributes.facet_transforms[0];
        *enabled_setting = GROUPING_PLUGIN_SETTING.into();
        value.contributes.menu_controls[0].checked_setting = Some(GROUPING_PLUGIN_SETTING.into());
        value.contributes.menu_controls[0].action = PluginMenuAction::ToggleSetting {
            setting: GROUPING_PLUGIN_SETTING.into(),
        };
        value
    }

    fn setup() -> (tempfile::TempDir, Arc<Db>, PluginManager) {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Db::open(&dir.path().join("library.db")).unwrap());
        let manager =
            PluginManager::start(dir.path().join("plugins"), db.clone(), "0.8.0").unwrap();
        (dir, db, manager)
    }

    #[test]
    fn first_install_is_disabled_and_does_not_request_restart() {
        let (dir, db, manager) = setup();
        let package = dir.path().join("plugin.crescendo-plugin");
        // A stale lifecycle row (for example after external directory removal)
        // must not turn a newly installed package on implicitly.
        db.plugin_set_desired_enabled("dev.crescendo.manager-test", true)
            .unwrap();
        write_test_package(&package, &manifest("1.0.0"), &[]);
        let result = manager.install(&package).unwrap();
        assert_eq!(result.snapshot.plugins.len(), 1);
        assert!(!result.snapshot.plugins[0].desired_enabled);
        assert!(!result.snapshot.plugins[0].loaded);
        assert!(!result.snapshot.restart_required);
    }

    #[test]
    fn enable_and_disable_compare_desired_state_with_the_loaded_registry() {
        let (dir, db, manager) = setup();
        let package = dir.path().join("plugin.crescendo-plugin");
        write_test_package(&package, &manifest("1.0.0"), &[]);
        manager.install(&package).unwrap();
        assert!(
            manager
                .set_enabled("dev.crescendo.manager-test", true)
                .unwrap()
                .snapshot
                .restart_required
        );
        let cancelled = manager
            .set_enabled("dev.crescendo.manager-test", false)
            .unwrap();
        assert!(!cancelled.snapshot.restart_required);
        assert!(cancelled.message.unwrap().contains("cancelled"));
        manager
            .set_enabled("dev.crescendo.manager-test", true)
            .unwrap();

        drop(manager);
        let restarted = PluginManager::start(dir.path().join("plugins"), db, "0.8.0").unwrap();
        let snapshot = restarted.snapshot().unwrap();
        assert!(snapshot.plugins[0].loaded);
        assert!(!snapshot.restart_required);
        assert!(
            restarted
                .set_enabled("dev.crescendo.manager-test", false)
                .unwrap()
                .snapshot
                .restart_required
        );
        let cancelled = restarted
            .set_enabled("dev.crescendo.manager-test", true)
            .unwrap();
        assert!(!cancelled.snapshot.restart_required);
        assert!(cancelled.message.unwrap().contains("cancelled"));
    }

    #[test]
    fn update_is_staged_then_promoted_on_startup() {
        let (dir, db, manager) = setup();
        let package = dir.path().join("plugin.crescendo-plugin");
        write_test_package(&package, &manifest("1.0.0"), &[]);
        manager.install(&package).unwrap();
        write_test_package(&package, &manifest("2.0.0"), &[]);
        let staged = manager.install(&package).unwrap();
        assert_eq!(staged.snapshot.plugins[0].version.as_deref(), Some("1.0.0"));
        assert_eq!(
            staged.snapshot.plugins[0]
                .pending
                .as_ref()
                .unwrap()
                .target_version
                .as_deref(),
            Some("2.0.0")
        );
        drop(manager);

        let restarted = PluginManager::start(dir.path().join("plugins"), db, "0.8.0").unwrap();
        assert_eq!(
            restarted.snapshot().unwrap().plugins[0].version.as_deref(),
            Some("2.0.0")
        );
    }

    #[test]
    fn updates_reject_same_or_lower_versions() {
        let (dir, db, manager) = setup();
        let package = dir.path().join("plugin.crescendo-plugin");
        write_test_package(&package, &manifest("2.0.0"), &[]);
        manager.install(&package).unwrap();

        for version in ["2.0.0", "1.9.9"] {
            write_test_package(&package, &manifest(version), &[]);
            assert!(manager.install(&package).is_err(), "accepted {version}");
        }
        assert!(db.plugin_pending_all().unwrap().is_empty());
        assert_eq!(
            manager.snapshot().unwrap().plugins[0].version.as_deref(),
            Some("2.0.0")
        );
    }

    #[test]
    fn loaded_uninstall_is_deferred_but_disabled_uninstall_is_immediate() {
        let (dir, db, manager) = setup();
        let package = dir.path().join("plugin.crescendo-plugin");
        write_test_package(&package, &manifest("1.0.0"), &[]);
        manager.install(&package).unwrap();
        let immediate = manager
            .uninstall("dev.crescendo.manager-test", false)
            .unwrap();
        assert!(immediate.snapshot.plugins.is_empty());

        manager.install(&package).unwrap();
        manager
            .set_enabled("dev.crescendo.manager-test", true)
            .unwrap();
        drop(manager);
        let restarted =
            PluginManager::start(dir.path().join("plugins"), db.clone(), "0.8.0").unwrap();
        let deferred = restarted
            .uninstall("dev.crescendo.manager-test", false)
            .unwrap();
        assert_eq!(
            deferred.snapshot.plugins[0]
                .pending
                .as_ref()
                .unwrap()
                .operation,
            "uninstall"
        );
        assert!(deferred.snapshot.restart_required);
        drop(restarted);
        let final_start = PluginManager::start(dir.path().join("plugins"), db, "0.8.0").unwrap();
        assert!(final_start.snapshot().unwrap().plugins.is_empty());
    }

    #[test]
    fn settings_validate_and_apply_without_a_restart() {
        let (dir, _db, manager) = setup();
        let package = dir.path().join("plugin.crescendo-plugin");
        write_test_package(&package, &manifest("1.0.0"), &[]);
        manager.install(&package).unwrap();
        let changed = manager
            .set_setting(
                "dev.crescendo.manager-test",
                "mode",
                Value::String("two".into()),
            )
            .unwrap();
        assert!(!changed.snapshot.restart_required);
        assert_eq!(
            changed.snapshot.plugins[0].settings[0].value,
            Value::String("two".into())
        );
        assert!(manager
            .set_setting(
                "dev.crescendo.manager-test",
                "mode",
                Value::String("invalid".into())
            )
            .is_err());
    }

    #[test]
    fn loaded_transform_settings_publish_one_revision_and_stale_guards_fail() {
        let (dir, db, manager) = setup();
        db.with(|connection| {
            for (id, grouping) in [(1, "Theme; Variant"), (2, "Theme; Other")] {
                connection.execute(
                    "INSERT INTO tracks(id, path, dir, filename, mtime, size, format, duration_ms, grouping)
                     VALUES (?1, ?2, '/m', ?3, 0, 0, 'FLAC', 1, ?4)",
                    rusqlite::params![id, format!("/m/{id}.flac"), format!("{id}.flac"), grouping],
                )?;
            }
            Ok(())
        })
        .unwrap();
        let package = dir.path().join("transform.crescendo-plugin");
        write_test_package(
            &package,
            &transform_manifest("dev.crescendo.transform-test"),
            &[],
        );
        manager.install(&package).unwrap();
        manager
            .set_enabled("dev.crescendo.transform-test", true)
            .unwrap();
        drop(manager);

        let manager =
            PluginManager::start(dir.path().join("plugins"), db.clone(), "0.8.0").unwrap();
        let mode = BrowseMode::Custom("grouping".into());
        let projection = manager.projection(Some(0)).unwrap();
        let plan = projection.facet_plan(&mode).unwrap();
        assert_eq!(
            db.browse_values_with_plan(Some(&plan), None).unwrap()[0].count,
            2
        );
        drop(projection);

        let mutation = manager
            .set_setting(
                "dev.crescendo.transform-test",
                "collapse_titles",
                Value::Bool(false),
            )
            .unwrap();
        assert_eq!(mutation.snapshot.projection_revision, 1);
        assert_eq!(mutation.snapshot.menu_controls[0].checked, Some(false));
        let stale = manager.projection(Some(0)).err().unwrap();
        assert!(stale.downcast_ref::<StaleProjection>().is_some());
        let projection = manager.projection(Some(1)).unwrap();
        let plan = projection.facet_plan(&mode).unwrap();
        assert_eq!(
            db.browse_values_with_plan(Some(&plan), None).unwrap().len(),
            2
        );
    }

    #[test]
    fn projection_publication_waits_for_an_in_flight_query_generation() {
        let (dir, db, manager) = setup();
        db.with(|connection| {
            for (id, grouping) in [(1, "Theme; A"), (2, "Theme; B")] {
                connection.execute(
                    "INSERT INTO tracks(id, path, dir, filename, mtime, size, format, duration_ms, grouping)
                     VALUES (?1, ?2, '/m', ?3, 0, 0, 'FLAC', 1, ?4)",
                    rusqlite::params![id, format!("/m/{id}.flac"), format!("{id}.flac"), grouping],
                )?;
            }
            Ok(())
        })
        .unwrap();
        let package = dir.path().join("transform.crescendo-plugin");
        write_test_package(
            &package,
            &transform_manifest("dev.crescendo.transform-lock"),
            &[],
        );
        manager.install(&package).unwrap();
        manager
            .set_enabled("dev.crescendo.transform-lock", true)
            .unwrap();
        drop(manager);
        let manager = Arc::new(
            PluginManager::start(dir.path().join("plugins"), db.clone(), "0.8.0").unwrap(),
        );
        let mode = BrowseMode::Custom("grouping".into());
        let old_projection = manager.projection(Some(0)).unwrap();
        let old_plan = old_projection.facet_plan(&mode).unwrap();

        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let worker = manager.clone();
        std::thread::spawn(move || {
            let result = worker.set_setting(
                "dev.crescendo.transform-lock",
                "collapse_titles",
                Value::Bool(false),
            );
            done_tx.send(result.is_ok()).unwrap();
        });
        for _ in 0..10_000 {
            if db
                .plugin_settings_get("dev.crescendo.transform-lock")
                .unwrap()
                .get("collapse_titles")
                == Some(&Value::Bool(false))
            {
                break;
            }
            std::thread::yield_now();
        }
        assert_eq!(
            db.plugin_settings_get("dev.crescendo.transform-lock")
                .unwrap()
                .get("collapse_titles"),
            Some(&Value::Bool(false))
        );
        assert_eq!(
            db.browse_values_with_plan(Some(&old_plan), None)
                .unwrap()
                .len(),
            1
        );
        assert!(done_rx.try_recv().is_err());
        drop(old_projection);
        assert!(done_rx.recv().unwrap());
        assert_eq!(manager.projection(Some(1)).unwrap().revision(), 1);
    }

    #[test]
    fn enabling_a_conflicting_facet_transform_is_rejected_transactionally() {
        let (dir, _db, manager) = setup();
        for id in ["dev.crescendo.transform-a", "dev.crescendo.transform-b"] {
            let package = dir.path().join(format!("{id}.crescendo-plugin"));
            write_test_package(&package, &transform_manifest(id), &[]);
            manager.install(&package).unwrap();
        }
        manager
            .set_enabled("dev.crescendo.transform-a", true)
            .unwrap();
        assert!(manager
            .set_enabled("dev.crescendo.transform-b", true)
            .is_err());
        let snapshot = manager.snapshot().unwrap();
        assert!(
            snapshot
                .plugins
                .iter()
                .find(|plugin| plugin.id == "dev.crescendo.transform-a")
                .unwrap()
                .desired_enabled
        );
        assert!(
            !snapshot
                .plugins
                .iter()
                .find(|plugin| plugin.id == "dev.crescendo.transform-b")
                .unwrap()
                .desired_enabled
        );
    }

    #[test]
    fn grouping_migration_is_opt_in_and_never_returns_after_install_or_dismiss() {
        let (dir, db, manager) = setup();
        db.meta_set(GROUPING_MIGRATION_KEY, "pending").unwrap();
        db.prefs_set("grouping_split_multi", "false").unwrap();
        let package = dir.path().join("grouping.crescendo-plugin");
        write_test_package(&package, &official_grouping_manifest(), &[]);

        let installed = manager
            .complete_grouping_migration(Some(&package))
            .unwrap();
        assert!(!installed.snapshot.grouping_migration_pending);
        let plugin = installed
            .snapshot
            .plugins
            .iter()
            .find(|plugin| plugin.id == GROUPING_PLUGIN_ID)
            .unwrap();
        assert!(plugin.desired_enabled);
        assert!(!plugin.loaded);
        assert_eq!(plugin.settings[1].value, Value::Bool(true));
        assert!(installed.snapshot.restart_required);
        assert!(db.prefs_get("grouping_split_multi").unwrap().is_none());
        assert!(!manager
            .complete_grouping_migration(None)
            .unwrap()
            .changed);

        let (dismiss_dir, dismiss_db, dismiss_manager) = setup();
        dismiss_db
            .meta_set(GROUPING_MIGRATION_KEY, "pending")
            .unwrap();
        let dismissed = dismiss_manager
            .complete_grouping_migration(None)
            .unwrap();
        assert!(!dismissed.snapshot.grouping_migration_pending);
        assert!(dismissed.snapshot.plugins.is_empty());
        drop(dismiss_manager);
        drop(dismiss_db);
        drop(dismiss_dir);
    }

    #[test]
    fn leftover_startup_marker_enters_safe_mode_for_one_run() {
        let (dir, db, manager) = setup();
        drop(manager);
        let root = dir.path().join("plugins");
        File::create(root.join(STARTUP_MARKER)).unwrap();
        let safe = PluginManager::start(root.clone(), db.clone(), "0.8.0").unwrap();
        assert!(safe.snapshot().unwrap().safe_mode);
        drop(safe);
        assert!(
            !PluginManager::start(root, db, "0.8.0")
                .unwrap()
                .snapshot()
                .unwrap()
                .safe_mode
        );
    }

    #[test]
    fn startup_resumes_an_update_interrupted_after_backing_up_the_old_copy() {
        let (dir, db, manager) = setup();
        let package = dir.path().join("plugin.crescendo-plugin");
        write_test_package(&package, &manifest("1.0.0"), &[]);
        manager.install(&package).unwrap();
        write_test_package(&package, &manifest("2.0.0"), &[]);
        manager.install(&package).unwrap();
        let pending = db.plugin_pending_all().unwrap().remove(0);
        drop(manager);

        let root = dir.path().join("plugins");
        let operation = root.join("staging").join(&pending.staged_path);
        fs::rename(
            root.join("installed/dev.crescendo.manager-test"),
            operation.join("old"),
        )
        .unwrap();

        let recovered = PluginManager::start(root, db, "0.8.0").unwrap();
        assert_eq!(
            recovered.snapshot().unwrap().plugins[0].version.as_deref(),
            Some("2.0.0")
        );
    }

    #[test]
    fn startup_commits_an_update_interrupted_after_promotion() {
        let (dir, db, manager) = setup();
        let package = dir.path().join("plugin.crescendo-plugin");
        write_test_package(&package, &manifest("1.0.0"), &[]);
        manager.install(&package).unwrap();
        write_test_package(&package, &manifest("2.0.0"), &[]);
        manager.install(&package).unwrap();
        let pending = db.plugin_pending_all().unwrap().remove(0);
        drop(manager);

        let root = dir.path().join("plugins");
        let operation = root.join("staging").join(&pending.staged_path);
        let installed = root.join("installed/dev.crescendo.manager-test");
        fs::rename(&installed, operation.join("old")).unwrap();
        fs::rename(operation.join("new"), &installed).unwrap();

        let recovered = PluginManager::start(root, db.clone(), "0.8.0").unwrap();
        assert_eq!(
            recovered.snapshot().unwrap().plugins[0].version.as_deref(),
            Some("2.0.0")
        );
        assert!(db.plugin_pending_all().unwrap().is_empty());
    }

    #[test]
    fn incomplete_update_rolls_back_instead_of_losing_the_installed_plugin() {
        let (dir, db, manager) = setup();
        let package = dir.path().join("plugin.crescendo-plugin");
        write_test_package(&package, &manifest("1.0.0"), &[]);
        manager.install(&package).unwrap();
        let root = dir.path().join("plugins");
        let (operation_id, operation) = create_operation_dir(&root.join("staging")).unwrap();
        fs::rename(
            root.join("installed/dev.crescendo.manager-test"),
            operation.join("old"),
        )
        .unwrap();
        db.plugin_schedule_pending(
            &PluginPendingRow {
                plugin_id: "dev.crescendo.manager-test".into(),
                operation: "update".into(),
                staged_path: operation_id,
                target_version: Some("2.0.0".into()),
                package_hash: Some("00".repeat(32)),
                remove_data: false,
            },
            None,
        )
        .unwrap();
        drop(manager);

        let recovered = PluginManager::start(root, db, "0.8.0").unwrap();
        let snapshot = recovered.snapshot().unwrap();
        assert_eq!(snapshot.plugins[0].version.as_deref(), Some("1.0.0"));
        assert!(snapshot.plugins[0].error.is_some());
    }

    #[test]
    fn startup_rejects_an_update_that_no_longer_matches_its_journal() {
        let (dir, db, manager) = setup();
        let package = dir.path().join("plugin.crescendo-plugin");
        write_test_package(&package, &manifest("1.0.0"), &[]);
        manager.install(&package).unwrap();
        manager
            .set_enabled("dev.crescendo.manager-test", true)
            .unwrap();
        drop(manager);
        let manager =
            PluginManager::start(dir.path().join("plugins"), db.clone(), "0.8.0").unwrap();
        write_test_package(&package, &manifest("2.0.0"), &[]);
        manager.install(&package).unwrap();
        let pending = db.plugin_pending_all().unwrap().remove(0);
        drop(manager);

        fs::write(
            dir.path()
                .join("plugins/staging")
                .join(pending.staged_path)
                .join("new/.crescendo-install.json"),
            format!(r#"{{"package_hash":"{}"}}"#, "00".repeat(32)),
        )
        .unwrap();

        let recovered = PluginManager::start(dir.path().join("plugins"), db, "0.8.0").unwrap();
        let snapshot = recovered.snapshot().unwrap();
        assert_eq!(snapshot.plugins[0].version.as_deref(), Some("1.0.0"));
        assert!(snapshot.plugins[0].error.is_some());
        assert!(snapshot.plugins[0].desired_enabled);
        assert!(snapshot.plugins[0].loaded);
        assert!(!snapshot.restart_required);
    }

    #[test]
    fn uninstall_preserves_settings_unless_data_removal_is_requested() {
        let (dir, _db, manager) = setup();
        let package = dir.path().join("plugin.crescendo-plugin");
        write_test_package(&package, &manifest("1.0.0"), &[]);
        manager.install(&package).unwrap();
        manager
            .set_setting(
                "dev.crescendo.manager-test",
                "mode",
                Value::String("two".into()),
            )
            .unwrap();
        manager
            .uninstall("dev.crescendo.manager-test", false)
            .unwrap();
        let restored = manager.install(&package).unwrap();
        assert_eq!(
            restored.snapshot.plugins[0].settings[0].value,
            Value::String("two".into())
        );

        manager
            .uninstall("dev.crescendo.manager-test", true)
            .unwrap();
        let clean = manager.install(&package).unwrap();
        assert_eq!(
            clean.snapshot.plugins[0].settings[0].value,
            Value::String("one".into())
        );
    }

    #[test]
    fn api_evolution_rules_decide_which_packages_this_host_accepts() {
        let app = Version::parse("0.8.0").expect("valid app version");
        let declarative = |api: &str| {
            let mut value = manifest("1.0.0");
            value.api_version = api.into();
            compatibility_error(&value, &app)
        };
        // Declarative API 1.x: the host accepts its own version and any older
        // compatible minor, and refuses anything newer or another major.
        assert!(declarative(PLUGIN_API_VERSION).is_none());
        assert!(declarative("1.0.0").is_none());
        assert!(declarative("1.99.0").is_some(), "newer minor than the host");
        assert!(declarative("2.0.0").is_some(), "another major");
        assert!(declarative("0.9.0").is_some(), "an older major");

        let wasm = |api: &str| {
            let mut value = manifest("1.0.0");
            value.kind = PluginKind::Wasm;
            value.contributes = PluginContributions::default();
            value.api_version = api.into();
            compatibility_error(&value, &app)
        };
        // Wasm runtime API 0.x: a differing minor is breaking because the
        // exported `commands` interface belongs to the world, while an older
        // patch is a compatible addition and still loads.
        assert!(wasm(WASM_API_VERSION).is_none());
        assert!(wasm("0.2.0").is_none(), "an older patch stays compatible");
        assert!(wasm("0.2.99").is_some(), "newer than the host");
        assert!(wasm("0.1.0").is_some(), "a 0.x minor is breaking");
        assert!(wasm("0.3.0").is_some(), "a 0.x minor is breaking");
        assert!(wasm("1.0.0").is_some(), "another major");

        // The two versions move independently: a Wasm package naming the
        // declarative API is refused, and the reverse too.
        assert!(wasm(PLUGIN_API_VERSION).is_some());
        assert!(declarative(WASM_API_VERSION).is_some());
    }

    #[test]
    fn incompatible_plugins_stay_installed_but_cannot_be_enabled() {
        let (dir, _db, manager) = setup();
        let mut too_new = manifest("1.0.0");
        too_new.min_crescendo_version = "99.0.0".into();
        let package = dir.path().join("plugin.crescendo-plugin");
        write_test_package(&package, &too_new, &[]);
        let installed = manager.install(&package).unwrap();
        assert!(!installed.snapshot.plugins[0].compatible);
        assert!(manager
            .set_enabled("dev.crescendo.manager-test", true)
            .is_err());
    }

    #[test]
    fn invalid_installed_plugin_is_reported_without_blocking_startup() {
        let (dir, db, manager) = setup();
        let package = dir.path().join("plugin.crescendo-plugin");
        write_test_package(&package, &manifest("1.0.0"), &[]);
        manager.install(&package).unwrap();
        manager
            .set_enabled("dev.crescendo.manager-test", true)
            .unwrap();
        drop(manager);

        fs::write(
            dir.path()
                .join("plugins/installed/dev.crescendo.manager-test/manifest.json"),
            b"not json",
        )
        .unwrap();
        let restarted = PluginManager::start(dir.path().join("plugins"), db, "0.8.0").unwrap();
        let snapshot = restarted.snapshot().unwrap();
        assert_eq!(snapshot.plugins.len(), 1);
        assert!(snapshot.loaded_plugin_ids.is_empty());
        assert!(snapshot.plugins[0].error.is_some());
        assert!(!snapshot.plugins[0].desired_enabled);
    }

    #[test]
    fn loaded_assets_are_resolved_by_id_and_never_by_package_path() {
        let (dir, db, manager) = setup();
        let package = dir.path().join("plugin.crescendo-plugin");
        write_test_package(
            &package,
            &manifest("1.0.0"),
            &[("assets/help/readme.txt", b"host-only", None)],
        );
        manager.install(&package).unwrap();
        assert!(manager
            .load_asset("dev.crescendo.manager-test", "help/readme.txt")
            .is_err());
        manager
            .set_enabled("dev.crescendo.manager-test", true)
            .unwrap();
        drop(manager);

        let restarted =
            PluginManager::start(dir.path().join("plugins"), db, "0.8.0").unwrap();
        let asset = restarted
            .load_asset("dev.crescendo.manager-test", "help/readme.txt")
            .unwrap();
        assert_eq!(asset.bytes, b"host-only");
        assert_eq!(asset.media_type, "text/plain; charset=utf-8");
        for id in ["../manifest.json", "/etc/passwd", "help/panel.html"] {
            assert!(restarted
                .load_asset("dev.crescendo.manager-test", id)
                .is_err());
        }
    }

    #[test]
    fn failed_startup_manager_is_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Db::open(&dir.path().join("library.db")).unwrap());
        let manager = PluginManager::disabled(
            dir.path().join("plugins"),
            db,
            "0.8.0",
            "startup failed".into(),
        );

        assert!(manager
            .install(&dir.path().join("missing.crescendo-plugin"))
            .is_err());
        assert!(manager
            .set_enabled("dev.crescendo.manager-test", true)
            .is_err());
        assert!(manager
            .set_setting(
                "dev.crescendo.manager-test",
                "mode",
                Value::String("two".into())
            )
            .is_err());
        assert!(manager
            .uninstall("dev.crescendo.manager-test", false)
            .is_err());
        assert!(manager.snapshot().unwrap().safe_mode);
    }
}
