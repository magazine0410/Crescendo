use super::manifest::{PluginKind, PluginManifest, PluginSettingValue};
use super::package::{
    create_operation_dir, load_installed, stage_archive, sync_directory, InstalledPackage,
};
use super::PLUGIN_API_VERSION;
use crate::db::plugins::PluginPendingRow;
use crate::Db;
use anyhow::{anyhow, bail, Context, Result};
use semver::Version;
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const STARTUP_MARKER: &str = "startup-in-progress";

#[derive(Debug, Clone, Serialize)]
pub struct PluginSnapshot {
    pub plugins: Vec<PluginInfo>,
    pub loaded_plugin_ids: Vec<String>,
    pub safe_mode: bool,
    pub safe_mode_reason: Option<String>,
    pub errors: Vec<String>,
    pub restart_required: bool,
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

pub struct PluginManager {
    installed_root: PathBuf,
    staging_root: PathBuf,
    db: Arc<Db>,
    app_version: Version,
    loaded: BTreeMap<String, PluginManifest>,
    available: bool,
    safe_mode: bool,
    safe_mode_reason: Option<String>,
    startup_errors: Vec<String>,
    operation_lock: Mutex<()>,
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
        let env_disabled =
            std::env::var_os("CRESCENDO_DISABLE_PLUGINS").is_some_and(|value| value == "1");
        let safe_mode = incomplete || env_disabled;
        let safe_mode_reason = if env_disabled {
            Some("Plugins were disabled by CRESCENDO_DISABLE_PLUGINS=1.".into())
        } else if incomplete {
            Some(
                "The previous plugin startup did not finish; plugins are disabled for this run."
                    .into(),
            )
        } else {
            None
        };

        let mut startup_errors = Vec::new();
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
                loaded.insert(record.id.clone(), package.manifest.clone());
            }
        }

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
            available: true,
            safe_mode,
            safe_mode_reason,
            startup_errors,
            operation_lock: Mutex::new(()),
        })
    }

    pub fn disabled(root: PathBuf, db: Arc<Db>, app_version: &str, error: String) -> Self {
        let app_version = Version::parse(app_version).unwrap_or_else(|_| Version::new(0, 0, 0));
        Self {
            installed_root: root.join("installed"),
            staging_root: root.join("staging"),
            db,
            app_version,
            loaded: BTreeMap::new(),
            available: false,
            safe_mode: true,
            safe_mode_reason: Some("The plugin system could not initialize.".into()),
            startup_errors: vec![error],
            operation_lock: Mutex::new(()),
        }
    }

    pub fn snapshot(&self) -> Result<PluginSnapshot> {
        let _guard = self
            .operation_lock
            .lock()
            .map_err(|_| anyhow!("plugin manager lock poisoned"))?;
        self.snapshot_unlocked()
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
                        PluginSettingValue { declaration, value }
                    })
                    .collect();
                plugins.push(PluginInfo {
                    id: record.id,
                    name: package.manifest.name.clone(),
                    version: Some(package.manifest.version.clone()),
                    description: package.manifest.description.clone(),
                    authors: package.manifest.authors.clone(),
                    license: Some(package.manifest.license.clone()),
                    homepage: package.manifest.homepage.clone(),
                    kind: Some(package.manifest.kind),
                    api_version: Some(package.manifest.api_version.clone()),
                    min_crescendo_version: Some(package.manifest.min_crescendo_version.clone()),
                    package_hash: Some(package.package_hash),
                    desired_enabled,
                    loaded,
                    compatible,
                    compatibility_error,
                    error: state_error,
                    pending: pending_status,
                    settings,
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
}

fn compatibility_error(manifest: &PluginManifest, app_version: &Version) -> Option<String> {
    if manifest.kind != PluginKind::Declarative {
        return Some("This build does not yet include the Wasm plugin runtime.".into());
    }
    let requested = Version::parse(&manifest.api_version).ok()?;
    let supported = Version::parse(PLUGIN_API_VERSION).expect("valid host plugin API version");
    if requested.major != supported.major || requested > supported {
        return Some(format!(
            "Requires plugin API {}, but this build supports {}.",
            manifest.api_version, PLUGIN_API_VERSION
        ));
    }
    let minimum = Version::parse(&manifest.min_crescendo_version).ok()?;
    if minimum > *app_version {
        return Some(format!("Requires Crescendo {minimum} or newer."));
    }
    None
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
            },
        }
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
