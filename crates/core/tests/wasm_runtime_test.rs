//! Stage 5 sandboxed Wasm runtime tests, run against the real Wasmtime
//! engine, the real database, and the real demo component built from
//! `plugins/dev.crescendo.wasm-demo` (fixture: `fixtures/wasm-demo-module.wasm`).

use crescendo_core::db::Db;
use crescendo_core::plugins::wasm::{
    HostEvent, RuntimeLimits, RuntimePaths, WasmHandle, WasmPluginSpec,
};
use crescendo_core::plugins::{
    PluginCommandDeclaration, PluginContributions, PluginKind, PluginManager, PluginManifest,
    PluginMenuAction, PluginMenuControl, PluginMenuSlot, PluginMenuVisibility,
    PluginSettingDeclaration, WASM_API_VERSION,
};
use serde_json::Value;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

const MODULE: &[u8] = include_bytes!("fixtures/wasm-demo-module.wasm");
/// The same demo component as it was built against runtime API 0.2.0, kept
/// unchanged so every later host keeps proving that a component built for an
/// older compatible API still loads. Never rebuild this file.
const LEGACY_MODULE: &[u8] = include_bytes!("fixtures/wasm-legacy-0.2.0-module.wasm");
const PLUGIN_ID: &str = "dev.crescendo.wasm-demo";

fn wasm_manifest(version: &str, permissions: &[&str]) -> PluginManifest {
    PluginManifest {
        schema_version: 1,
        id: PLUGIN_ID.into(),
        name: "Wasm Demo".into(),
        version: version.into(),
        description: "Runtime test plugin".into(),
        authors: vec!["Crescendo".into()],
        license: "MIT".into(),
        homepage: None,
        kind: PluginKind::Wasm,
        // The committed component is built against the current WIT, so the
        // manifest must name the current runtime API. A test that pinned an
        // older version here would package a component whose imports and
        // exports disagree with what it declares.
        api_version: WASM_API_VERSION.into(),
        min_crescendo_version: "0.8.0".into(),
        permissions: permissions.iter().map(|value| (*value).to_string()).collect(),
        network_domains: vec![],
        contributes: PluginContributions {
            settings: vec![PluginSettingDeclaration::Boolean {
                key: "demo_flag".into(),
                label: "Demo flag".into(),
                description: String::new(),
                default: false,
            }],
            commands: [
                "identity",
                "settings",
                "storage_roundtrip",
                "library_summary",
                "player_state",
                "player_stop",
                "playlists_all",
                "playlist_create",
                "net_fetch",
                "spin",
            ]
                .iter()
                .map(|id| PluginCommandDeclaration { id: (*id).into() })
                .collect(),
            ..PluginContributions::default()
        },
    }
}

fn write_package(path: &Path, manifest: &PluginManifest, module: &[u8]) {
    let file = File::create(path).unwrap();
    let mut writer = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    writer.start_file("manifest.json", options).unwrap();
    writer
        .write_all(&serde_json::to_vec(manifest).unwrap())
        .unwrap();
    writer.start_file("module.wasm", options).unwrap();
    writer.write_all(module).unwrap();
    writer.finish().unwrap();
}

fn setup() -> (TempDir, Arc<Db>, PluginManager) {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Db::open(&dir.path().join("library.db")).unwrap());
    db.with(|connection| {
        for id in 1..=2 {
            connection.execute(
                "INSERT INTO tracks(id, path, dir, filename, mtime, size, format, duration_ms, title, artist, album)
                 VALUES (?1, ?2, '/m', ?3, 0, 0, 'FLAC', 1000, ?4, 'Artist', 'Album')",
                rusqlite::params![id, format!("/m/{id}.flac"), format!("{id}.flac"), format!("Title {id}")],
            )?;
        }
        Ok(())
    })
    .unwrap();
    let manager =
        PluginManager::start(dir.path().join("plugins"), db.clone(), "0.8.0").unwrap();
    (dir, db, manager)
}

fn restart(dir: &TempDir, db: Arc<Db>) -> PluginManager {
    PluginManager::start(dir.path().join("plugins"), db, "0.8.0").unwrap()
}

fn plugin_info(manager: &PluginManager) -> crescendo_core::plugins::PluginInfo {
    manager
        .snapshot()
        .unwrap()
        .plugins
        .into_iter()
        .find(|plugin| plugin.id == PLUGIN_ID)
        .unwrap()
}

/// The runtime thread instantiates and runs `init` asynchronously, so wait
/// for readiness by polling a cheap command.
fn invoke_once_ready(manager: &PluginManager, command: &str, input: Value) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match manager.invoke(PLUGIN_ID, command, &input) {
            Ok(value) => return value,
            Err(error) => {
                let message = format!("{error:#}");
                assert!(
                    message.contains("failed to initialize")
                        || message.contains("not running")
                        || message.contains("no running Wasm instance"),
                    "unexpected invoke failure: {message}"
                );
                if Instant::now() > deadline {
                    panic!("plugin never became ready: {message}");
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    }
}

fn install_and_enable(manager: &PluginManager, dir: &TempDir, manifest: &PluginManifest) {
    let package = dir.path().join("plugin.crescendo-plugin");
    write_package(&package, manifest, MODULE);
    manager.install(&package).unwrap();
}

#[test]
fn a_component_built_against_an_older_compatible_api_still_loads() {
    // The world's imports are versioned, so this is the test that a host
    // patch release does not strand every installed component: the 0.2.0
    // component imports `crescendo:host/*@0.2.0` while this host provides
    // @0.2.1, and Wasmtime resolves the difference by semver.
    let (dir, db, manager) = setup();
    let mut manifest = wasm_manifest("1.0.0", &["library.read", "storage"]);
    manifest.api_version = "0.2.0".into();
    let package = dir.path().join("legacy.crescendo-plugin");
    write_package(&package, &manifest, LEGACY_MODULE);
    manager.install(&package).unwrap();
    assert!(
        plugin_info(&manager).compatible,
        "an older compatible API must stay installable: {:?}",
        plugin_info(&manager).compatibility_error
    );
    manager.approve_grants(PLUGIN_ID).unwrap();
    manager.set_enabled(PLUGIN_ID, true).unwrap();
    drop(manager);

    let manager = restart(&dir, db);
    let info = plugin_info(&manager);
    assert!(info.loaded, "expected the legacy component to load: {:?}", info.error);
    let summary = invoke_once_ready(&manager, "library_summary", serde_json::json!({}));
    assert_eq!(summary["total"], 2, "the legacy component still reaches the host");
    assert!(plugin_info(&manager).runtime_error.is_none());
}

#[test]
fn a_run_command_menu_control_reaches_the_snapshot_and_dispatches() {
    let (dir, db, manager) = setup();
    let mut manifest = wasm_manifest("1.0.0", &["library.read", "storage"]);
    manifest.contributes.menu_controls.push(PluginMenuControl {
        id: "log_library_summary".into(),
        slot: PluginMenuSlot::LibraryFilter,
        label: "Log a library summary (Wasm demo)".into(),
        visible_when: PluginMenuVisibility {
            active_tag: "composer".into(),
        },
        checked_setting: None,
        action: PluginMenuAction::RunCommand {
            command: "library_summary".into(),
        },
    });
    install_and_enable(&manager, &dir, &manifest);
    manager.approve_grants(PLUGIN_ID).unwrap();
    manager.set_enabled(PLUGIN_ID, true).unwrap();
    drop(manager);

    let manager = restart(&dir, db);
    let controls = manager.snapshot().unwrap().menu_controls;
    let control = controls
        .iter()
        .find(|control| control.id == "log_library_summary")
        .expect("the command action is contributed");
    assert_eq!(control.action, "run_command");
    assert_eq!(control.command.as_deref(), Some("library_summary"));
    // A command action carries no setting and no tick: there is no state for
    // the host to render.
    assert!(control.setting_key.is_none());
    assert!(control.checked.is_none());

    // The item dispatches through the one bounded path, exactly like
    // `plugin_invoke`.
    let result = invoke_once_ready(&manager, control.command.as_deref().unwrap(), serde_json::json!({}));
    assert_eq!(result["total"], 2);
}

/// An update replaces a copy that worked, and its backup is deleted as soon as
/// startup commits the journal. A package that is structurally valid but
/// carries a component this build cannot load must therefore be caught before
/// that commit, not later on the runtime thread.
#[test]
fn an_update_carrying_an_unloadable_component_is_rolled_back() {
    let (dir, db, manager) = setup();
    install_and_enable(&manager, &dir, &wasm_manifest("1.0.0", &[]));
    manager.set_enabled(PLUGIN_ID, true).unwrap();
    drop(manager);

    let manager = restart(&dir, db.clone());
    assert!(plugin_info(&manager).loaded);
    invoke_once_ready(&manager, "identity", serde_json::json!({}));

    // A well-formed package whose module.wasm is not a component at all.
    let update = dir.path().join("broken-update.crescendo-plugin");
    write_package(&update, &wasm_manifest("2.0.0", &[]), b"this is not a component");
    manager.install(&update).unwrap();
    assert_eq!(
        plugin_info(&manager).pending.as_ref().map(|pending| pending.operation.as_str()),
        Some("update"),
        "the update should be journalled for the next start"
    );
    drop(manager);

    let manager = restart(&dir, db);
    let info = plugin_info(&manager);
    assert_eq!(
        info.version.as_deref(),
        Some("1.0.0"),
        "the working version must survive an unloadable update"
    );
    assert!(info.pending.is_none(), "the journal entry should be resolved");
    assert!(
        info.error.as_deref().unwrap_or("").contains("could not be loaded"),
        "the rollback should explain itself: {:?}",
        info.error
    );
    assert!(info.loaded, "the restored version must still load");
    assert_eq!(
        invoke_once_ready(&manager, "identity", serde_json::json!({"n": 1}))["n"],
        1,
        "the restored component must still run"
    );
}

#[test]
fn a_component_built_against_a_breaking_api_is_refused_before_it_runs() {
    let (dir, _db, manager) = setup();
    let mut manifest = wasm_manifest("1.0.0", &[]);
    manifest.api_version = "0.1.0".into();
    let package = dir.path().join("old.crescendo-plugin");
    write_package(&package, &manifest, LEGACY_MODULE);
    manager.install(&package).unwrap();
    let info = plugin_info(&manager);
    assert!(!info.compatible);
    assert!(
        info.compatibility_error
            .as_deref()
            .unwrap_or_default()
            .contains("Wasm runtime API"),
        "expected an API explanation: {:?}",
        info.compatibility_error
    );
    assert!(manager.set_enabled(PLUGIN_ID, true).is_err());
}

#[test]
fn wasm_plugins_require_grant_approval_then_invoke_with_granted_capabilities() {
    let (dir, db, manager) = setup();
    install_and_enable(&manager, &dir, &wasm_manifest("1.0.0", &["library.read", "storage"]));

    // Install alone: disabled, capabilities pending approval, no restart yet.
    let info = plugin_info(&manager);
    assert!(!info.loaded);
    assert!(!info.desired_enabled);
    assert!(!info.grants_approved);
    assert_eq!(info.permissions, ["library.read", "storage"]);
    // identity, settings, storage_roundtrip, library_summary, the four
    // capability probes, and spin.
    assert_eq!(info.commands.len(), 10);

    // Enabling before approval is rejected; approval is a separate act.
    assert!(manager.set_enabled(PLUGIN_ID, true).is_err());
    manager.approve_grants(PLUGIN_ID).unwrap();
    assert!(plugin_info(&manager).grants_approved);
    manager.set_enabled(PLUGIN_ID, true).unwrap();

    drop(manager);
    let manager = restart(&dir, db.clone());
    let info = plugin_info(&manager);
    assert!(info.loaded, "expected the plugin to load: {:?}", info.error);

    // Host-visible identity round trip: input JSON comes back untouched.
    let input = serde_json::json!({"hello": "world", "n": 1});
    assert_eq!(invoke_once_ready(&manager, "identity", input.clone()), input);

    // On-demand live settings read.
    let settings = invoke_once_ready(&manager, "settings", serde_json::json!({}));
    assert_eq!(settings["count"], 1);
    assert_eq!(settings["keys"][0], "demo_flag");

    // Granted storage round-trips through the namespaced table.
    let stored = invoke_once_ready(
        &manager,
        "storage_roundtrip",
        serde_json::json!({"value": "abc"}),
    );
    assert_eq!(stored, serde_json::json!({"stored": "abc"}));
    assert_eq!(
        db.plugin_storage_get(PLUGIN_ID, "demo-key").unwrap(),
        Some("abc".into())
    );

    // Granted library read sees windowed, redacted metadata (no
    // library.paths in the manifest, so no path is returned).
    let summary = invoke_once_ready(&manager, "library_summary", serde_json::json!({}));
    assert_eq!(summary["total"], 2);
    assert_eq!(summary["first_title"], "Title 1");

    // Only registered commands are dispatchable.
    assert!(manager.invoke(PLUGIN_ID, "not-declared", &Value::Null).is_err());
    assert!(manager
        .invoke("dev.crescendo.unknown", "identity", &Value::Null)
        .is_err());

    // Init wrote to the bounded diagnostic log.
    let info = plugin_info(&manager);
    assert!(info
        .diagnostics
        .iter()
        .any(|entry| entry.contains("demo initialized")));
    assert!(info.runtime_error.is_none());
}

#[test]
fn capability_calls_are_denied_without_the_matching_permission() {
    let (dir, _db, manager) = setup();
    // The module imports storage and library, but the manifest requests no
    // capabilities, so no grant is needed — the host denies the calls.
    install_and_enable(&manager, &dir, &wasm_manifest("1.0.0", &[]));
    manager.set_enabled(PLUGIN_ID, true).unwrap();

    drop(manager);
    let manager = restart(&dir, _db.clone());
    assert!(plugin_info(&manager).loaded);

    let stored = invoke_once_ready(
        &manager,
        "storage_roundtrip",
        serde_json::json!({"value": "abc"}),
    );
    assert_eq!(stored, serde_json::json!({"denied": true}));
    let summary = invoke_once_ready(&manager, "library_summary", serde_json::json!({}));
    assert_eq!(summary, serde_json::json!({"denied": true}));
    let input = serde_json::json!({"still": "works"});
    assert_eq!(invoke_once_ready(&manager, "identity", input.clone()), input);
    assert_eq!(_db.plugin_storage_usage(PLUGIN_ID).unwrap(), 0);
}

#[test]
fn grants_are_pinned_to_the_exact_package_version_and_hash() {
    let (dir, db, manager) = setup();
    install_and_enable(&manager, &dir, &wasm_manifest("1.0.0", &["storage"]));
    manager.approve_grants(PLUGIN_ID).unwrap();
    manager.set_enabled(PLUGIN_ID, true).unwrap();
    drop(manager);

    let manager = restart(&dir, db.clone());
    assert!(plugin_info(&manager).loaded);

    // An update is a different package; the old approval must not carry it
    // over silently. The update applies after restart, and the plugin stays
    // unloaded until the user approves the new package.
    let package = dir.path().join("update.crescendo-plugin");
    write_package(&package, &wasm_manifest("2.0.0", &["storage"]), MODULE);
    manager.install(&package).unwrap();
    drop(manager);

    let manager = restart(&dir, db.clone());
    let info = plugin_info(&manager);
    assert!(!info.loaded, "the update must not load without re-approval");
    assert!(info.desired_enabled);
    assert!(!info.grants_approved);
    assert_eq!(info.version.as_deref(), Some("2.0.0"));
    assert!(info.runtime_error.is_none());

    manager.approve_grants(PLUGIN_ID).unwrap();
    manager.set_enabled(PLUGIN_ID, true).unwrap();
    drop(manager);

    let manager = restart(&dir, db);
    let info = plugin_info(&manager);
    assert!(info.loaded, "expected v2 to load after approval: {:?}", info.error);
    let input = serde_json::json!({"v": 2});
    assert_eq!(invoke_once_ready(&manager, "identity", input.clone()), input);
}

#[test]
fn guest_work_is_bounded_by_the_epoch_deadline_and_failures_mark_it_unhealthy() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Db::open(&dir.path().join("library.db")).unwrap());
    let module_path = dir.path().join("module.wasm");
    std::fs::write(&module_path, MODULE).unwrap();
    let spec = WasmPluginSpec {
        id: PLUGIN_ID.into(),
        module_path,
        manifest: Arc::new(wasm_manifest("1.0.0", &[])),
    };
    let limits = RuntimeLimits {
        fuel: 2_000_000_000,
        epoch_ticks: 3,
        invoke_timeout: Duration::from_secs(2),
    };
    let handle = WasmHandle::spawn(
        vec![spec],
        db,
        limits,
        Default::default(),
        Default::default(),
        RuntimePaths { safe_mode_marker: dir.path().join("safe-mode-requested") },
    )
    .unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    while !handle.runtime_info(PLUGIN_ID).ready {
        if Instant::now() > deadline {
            panic!("plugin never initialized");
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    // A spinning guest hits the epoch deadline quickly and fails cleanly,
    // and only the third session failure marks the plugin unhealthy.
    let started = Instant::now();
    assert!(handle.invoke(PLUGIN_ID, "spin", "{}").is_err());
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the epoch deadline must bound wall-clock time"
    );
    assert!(!handle.runtime_info(PLUGIN_ID).unhealthy);
    assert!(handle.invoke(PLUGIN_ID, "spin", "{}").is_err());
    assert!(!handle.runtime_info(PLUGIN_ID).unhealthy);
    assert!(handle.invoke(PLUGIN_ID, "spin", "{}").is_err());
    let info = handle.runtime_info(PLUGIN_ID);
    assert!(info.unhealthy, "expected the plugin to become unhealthy");
    assert!(info.error.as_deref().unwrap_or("").contains("unhealthy"));

    // Unhealthy plugins fail fast without touching guest code again.
    let failure = handle.invoke(PLUGIN_ID, "identity", "{}").unwrap_err();
    assert!(format!("{failure:#}").contains("unhealthy"));

    // And events stop reaching it. The demo logs from every handler, so a
    // delivered event shows up as a diagnostic; an invoke afterwards is the
    // sync point, because the runtime queue is FIFO.
    let before = handle.runtime_info(PLUGIN_ID).diagnostics.len();
    for _ in 0..5 {
        handle.notify_event(HostEvent::LibraryChanged);
        handle.notify_event(HostEvent::TrackChanged(Some(1)));
    }
    let _ = handle.invoke(PLUGIN_ID, "identity", "{}");
    assert_eq!(
        handle.runtime_info(PLUGIN_ID).diagnostics.len(),
        before,
        "an unhealthy plugin must not keep running event handlers"
    );
}

/// The counterpart to the assertion above: while a plugin is healthy the
/// events really are delivered, so the unhealthy case is not passing for want
/// of a working event path.
#[test]
fn events_reach_a_healthy_plugin() {
    let harness = Harness::new(&[]);
    let before = harness.handle.runtime_info(PLUGIN_ID).diagnostics.len();
    harness.handle.notify_event(HostEvent::LibraryChanged);
    let _ = harness.handle.invoke(PLUGIN_ID, "identity", "{}");
    assert!(
        harness.handle.runtime_info(PLUGIN_ID).diagnostics.len() > before,
        "a healthy plugin should have handled the event"
    );
}

#[test]
fn guest_work_is_bounded_by_fuel() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Db::open(&dir.path().join("library.db")).unwrap());
    let module_path = dir.path().join("module.wasm");
    std::fs::write(&module_path, MODULE).unwrap();
    let spec = WasmPluginSpec {
        id: PLUGIN_ID.into(),
        module_path,
        manifest: Arc::new(wasm_manifest("1.0.0", &[])),
    };
    let limits = RuntimeLimits {
        fuel: 50_000,
        epoch_ticks: 100_000,
        invoke_timeout: Duration::from_secs(5),
    };
    let handle = WasmHandle::spawn(
        vec![spec],
        db,
        limits,
        Default::default(),
        Default::default(),
        RuntimePaths { safe_mode_marker: dir.path().join("safe-mode-requested") },
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !handle.runtime_info(PLUGIN_ID).ready {
        if Instant::now() > deadline {
            panic!("plugin never initialized");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(handle.invoke(PLUGIN_ID, "spin", "{}").is_err());
    assert!(handle.runtime_info(PLUGIN_ID).error.is_some());
}

#[test]
fn a_broken_module_fails_isolated_and_auto_disables_after_repeated_startups() {
    let (dir, db, manager) = setup();
    let package = dir.path().join("broken.crescendo-plugin");
    write_package(&package, &wasm_manifest("1.0.0", &[]), b"this is not wasm");
    manager.install(&package).unwrap();
    manager.set_enabled(PLUGIN_ID, true).unwrap();
    drop(manager);

    // First startup failure: reported, desired state kept, playback of the
    // rest of the system unaffected. The runtime thread reports
    // asynchronously, so wait for the failure to surface.
    let manager = restart(&dir, db.clone());
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let info = plugin_info(&manager);
        assert!(info.loaded, "the registry entry loads; only the runtime fails");
        if info.runtime_error.is_some() {
            break;
        }
        assert!(Instant::now() < deadline, "the broken module never failed");
        std::thread::sleep(Duration::from_millis(25));
    };
    assert!(manager.invoke(PLUGIN_ID, "identity", &Value::Null).is_err());
    drop(manager);

    // Second startup failure: the plugin auto-disables so a broken package
    // can never wedge every startup.
    let manager = restart(&dir, db.clone());
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let info = plugin_info(&manager);
        if info.runtime_error.as_deref().unwrap_or("").contains("Disabled") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the repeated failure never auto-disabled the plugin: {:?}",
            info.runtime_error
        );
        std::thread::sleep(Duration::from_millis(25));
    };
    drop(manager);

    // The auto-disable also asks for one cautious run: the next start reports
    // safe mode, and consumes the request so it does not repeat.
    let manager = restart(&dir, db.clone());
    assert!(
        manager.snapshot().unwrap().safe_mode,
        "the second startup failure should request safe mode for the next run"
    );
    let info = plugin_info(&manager);
    assert!(!info.desired_enabled, "expected the plugin to stay disabled");
    assert!(!info.loaded);
    drop(manager);

    let manager = restart(&dir, db);
    assert!(
        !manager.snapshot().unwrap().safe_mode,
        "safe mode should last exactly one run"
    );
    let info = plugin_info(&manager);
    assert!(!info.desired_enabled, "expected the plugin to stay disabled");
    assert!(!info.loaded);
}

#[test]
fn settings_changes_reach_the_component_through_the_coalesced_notification() {
    let (dir, db, manager) = setup();
    install_and_enable(&manager, &dir, &wasm_manifest("1.0.0", &[]));
    manager.set_enabled(PLUGIN_ID, true).unwrap();
    drop(manager);

    let manager = restart(&dir, db);
    invoke_once_ready(&manager, "identity", serde_json::json!({}));
    manager
        .set_setting(PLUGIN_ID, "demo_flag", Value::Bool(true))
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let info = plugin_info(&manager);
        if info
            .diagnostics
            .iter()
            .any(|entry| entry.contains("settings changed"))
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the settings-changed notification never arrived: {:?}",
            info.diagnostics
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// A runtime with one demo plugin, for the capability tests. Drives the
/// `WasmHandle` directly: these assertions are about the host interfaces, not
/// about install and approval, which the lifecycle tests already cover.
struct Harness {
    _dir: tempfile::TempDir,
    db: Arc<Db>,
    handle: WasmHandle,
    player: Arc<crescendo_core::plugins::PlayerBridgeSlot>,
}

/// A player that reports something distinguishable and accepts every command.
struct FakePlayer;

impl crescendo_core::plugins::PlayerBridge for FakePlayer {
    fn state(&self) -> crescendo_core::plugins::BridgePlayerState {
        crescendo_core::plugins::BridgePlayerState {
            playing: true,
            volume: 0.75,
            ..Default::default()
        }
    }

    fn control(
        &self,
        _command: crescendo_core::plugins::BridgePlayerCommand,
    ) -> Result<(), String> {
        Ok(())
    }
}

impl Harness {
    fn new(permissions: &[&str]) -> Self {
        Self::with_domains(permissions, &[])
    }

    fn with_domains(permissions: &[&str], domains: &[&str]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Db::open(&dir.path().join("library.db")).unwrap());
        db.with(|connection| {
            for id in 1..=2 {
                connection.execute(
                    "INSERT INTO tracks(id, path, dir, filename, mtime, size, format, duration_ms, title, artist, album)
                     VALUES (?1, ?2, '/m', ?3, 0, 0, 'FLAC', 1000, ?4, 'Artist', 'Album')",
                    rusqlite::params![id, format!("/m/{id}.flac"), format!("{id}.flac"), format!("Title {id}")],
                )?;
            }
            Ok(())
        })
        .unwrap();
        let module_path = dir.path().join("module.wasm");
        std::fs::write(&module_path, MODULE).unwrap();
        let mut manifest = wasm_manifest("1.0.0", permissions);
        manifest.network_domains = domains.iter().map(|d| (*d).to_string()).collect();
        let spec = WasmPluginSpec {
            id: PLUGIN_ID.into(),
            module_path,
            manifest: Arc::new(manifest),
        };
        let player = Arc::new(crescendo_core::plugins::PlayerBridgeSlot::new());
        let handle = WasmHandle::spawn(
            vec![spec],
            db.clone(),
            RuntimeLimits::default(),
            player.clone(),
            Default::default(),
            RuntimePaths { safe_mode_marker: dir.path().join("safe-mode-requested") },
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !handle.runtime_info(PLUGIN_ID).ready {
            if Instant::now() > deadline {
                panic!("plugin never initialized");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Self { _dir: dir, db, handle, player }
    }

    fn install_player(&self) {
        self.player.install(Arc::new(FakePlayer));
    }

    fn invoke(&self, command: &str, input: &str) -> String {
        self.handle
            .invoke(PLUGIN_ID, command, input)
            .unwrap_or_else(|error| panic!("{command} failed: {error:#}"))
    }
}

/// `player.read` and `player.control` are separate capabilities, and neither
/// works before the shell installs its player: a headless host answers
/// "unavailable" rather than pretending playback exists.
#[test]
fn player_capabilities_are_separate_and_need_an_installed_player() {
    let harness = Harness::new(&["player.read"]);

    assert_eq!(harness.invoke("player_state", "{}"), "unavailable");
    // Granted read, but control was never requested.
    assert_eq!(harness.invoke("player_stop", "{}"), "denied");

    let none = Harness::new(&[]);
    assert_eq!(none.invoke("player_state", "{}"), "denied");

    let both = Harness::new(&["player.read", "player.control"]);
    both.install_player();
    assert_eq!(both.invoke("player_stop", "{}"), "ok");
    assert!(
        both.invoke("player_state", "{}").contains("\"playing\":true"),
        "an installed player must report its real state"
    );

    // Enqueueing is the one transport call carrying plugin-supplied data, so
    // the host checks it: a real id is queued, an id that names no track is
    // refused rather than left sitting in the user's queue.
    assert_eq!(both.invoke("player_enqueue", "{\"id\":\"1\"}"), "ok");
    assert_eq!(both.invoke("player_enqueue", "{\"id\":\"4242\"}"), "invalid");

    // Denial comes before validation, so an ungranted plugin cannot use the
    // enqueue error to probe which track ids exist.
    assert_eq!(harness.invoke("player_enqueue", "{\"id\":\"1\"}"), "denied");
    assert_eq!(harness.invoke("player_enqueue", "{\"id\":\"4242\"}"), "denied");
}

/// Namespaced storage is per plugin: identity comes from the store the call
/// arrived on, never from the call, so two plugins writing the same key cannot
/// see or overwrite each other. The quota is charged per plugin too.
#[test]
fn storage_is_namespaced_per_plugin_and_bounded_by_the_quota() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Db::open(&dir.path().join("library.db")).unwrap());
    let module_path = dir.path().join("module.wasm");
    std::fs::write(&module_path, MODULE).unwrap();

    let second_id = "dev.crescendo.wasm-demo-two";
    let specs = ["dev.crescendo.wasm-demo", second_id]
        .iter()
        .map(|id| {
            let mut manifest = wasm_manifest("1.0.0", &["storage"]);
            manifest.id = (*id).into();
            WasmPluginSpec {
                id: (*id).into(),
                module_path: module_path.clone(),
                manifest: Arc::new(manifest),
            }
        })
        .collect();
    let handle = WasmHandle::spawn(
        specs,
        db.clone(),
        RuntimeLimits::default(),
        Default::default(),
        Default::default(),
        RuntimePaths { safe_mode_marker: dir.path().join("safe-mode-requested") },
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !handle.runtime_info(second_id).ready {
        if Instant::now() > deadline {
            panic!("the second plugin never initialized");
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    // Both write the same key. Each reads back its own value.
    assert_eq!(
        handle.invoke(PLUGIN_ID, "storage_roundtrip", "{\"value\":\"first\"}").unwrap(),
        "{\"stored\":\"first\"}"
    );
    assert_eq!(
        handle.invoke(second_id, "storage_roundtrip", "{\"value\":\"second\"}").unwrap(),
        "{\"stored\":\"second\"}"
    );
    assert_eq!(
        handle.invoke(PLUGIN_ID, "storage_roundtrip", "{\"value\":\"first\"}").unwrap(),
        "{\"stored\":\"first\"}",
        "the second plugin must not have overwritten the first"
    );
    assert_eq!(
        db.plugin_storage_get(PLUGIN_ID, "demo-key").unwrap().as_deref(),
        Some("first")
    );
    assert_eq!(
        db.plugin_storage_get(second_id, "demo-key").unwrap().as_deref(),
        Some("second")
    );

    // Fill one plugin's namespace to its quota. The guest's next write is
    // refused, and the other plugin is unaffected: the quota is namespaced
    // the same way the values are.
    let block = "x".repeat(256 * 1024);
    for index in 0..21 {
        db.plugin_storage_set(PLUGIN_ID, &format!("filler{index}"), &block)
            .unwrap();
    }
    assert!(db.plugin_storage_usage(PLUGIN_ID).unwrap() > 5 * 1024 * 1024 - 256 * 1024);
    assert_eq!(
        handle.invoke(PLUGIN_ID, "storage_roundtrip", "{\"value\":\"overflow\"}").unwrap(),
        "{\"quota\":true}"
    );
    assert_eq!(
        handle.invoke(second_id, "storage_roundtrip", "{\"value\":\"still fine\"}").unwrap(),
        "{\"stored\":\"still fine\"}"
    );
}

/// Playlist reads and writes are separate, and a write really lands.
#[test]
fn playlist_reads_and_writes_are_separately_gated() {
    let denied = Harness::new(&["playlists.read"]);
    assert_eq!(denied.invoke("playlists_all", "{}"), "{\"count\":0}");
    assert_eq!(
        denied.invoke("playlist_create", "{\"name\":\"From a plugin\"}"),
        "denied"
    );

    let allowed = Harness::new(&["playlists.read", "playlists.write"]);
    let created = allowed.invoke("playlist_create", "{\"name\":\"From a plugin\"}");
    assert!(created.starts_with("{\"id\":"), "unexpected: {created}");
    assert_eq!(allowed.invoke("playlists_all", "{}"), "{\"count\":1}");
    assert_eq!(
        allowed.db.playlist_list().unwrap()[0].name,
        "From a plugin",
        "the write must reach the real database"
    );
}

/// Network access is refused without the grant, refused for a host outside the
/// approved list, and refused for an approved host that resolves somewhere
/// private. None of these opens a socket.
#[test]
fn network_requests_are_denied_blocked_or_allowed_by_host_not_by_the_guest() {
    let none = Harness::new(&[]);
    assert_eq!(
        none.invoke("net_fetch", "{\"url\":\"https://example.com/\"}"),
        "denied"
    );

    let granted = Harness::with_domains(&["network.http"], &["approved.test"]);
    // Approved permission, unapproved host.
    assert_eq!(
        granted.invoke("net_fetch", "{\"url\":\"https://example.com/\"}"),
        "blocked"
    );
    // Plain http is refused before anything resolves.
    assert_eq!(
        granted.invoke("net_fetch", "{\"url\":\"http://approved.test/\"}"),
        "invalid"
    );

    // An approved name that points at loopback is still refused.
    let loopback = Harness::with_domains(&["network.http"], &["localhost"]);
    assert_eq!(
        loopback.invoke("net_fetch", "{\"url\":\"https://localhost/\"}"),
        "blocked"
    );
}

/// A `playlists.write` plugin supplies raw numbers. Unknown ids must be
/// refused, not persisted: `playlist_tracks` carries no foreign keys, so a bad
/// insert would leave rows pointing at nothing forever.
#[test]
fn playlist_writes_refuse_ids_that_do_not_exist() {
    let harness = Harness::new(&["playlists.read", "playlists.write"]);
    let created = harness.invoke("playlist_create", "{\"name\":\"Real\"}");
    assert!(created.starts_with("{\"id\":"), "unexpected: {created}");

    // A playlist that does not exist.
    assert!(harness
        .db
        .playlist_add_tracks_checked(9_999, &[1])
        .is_err());
    // A real playlist, but a track that does not exist.
    let id = harness.db.playlist_list().unwrap()[0].id;
    assert!(harness.db.playlist_add_tracks_checked(id, &[4_242]).is_err());
    assert!(
        harness.db.playlist_track_ids_page(id, 0, 100).unwrap().is_empty(),
        "a refused insert must leave nothing behind"
    );
}
