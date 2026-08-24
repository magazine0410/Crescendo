use std::{env, fs, path::Path};

fn cargo_version(lock: &str, package: &str) -> String {
    let marker = format!("name = \"{package}\"");
    let package_block = lock
        .split("[[package]]")
        .find(|block| block.lines().any(|line| line.trim() == marker))
        .unwrap_or_else(|| panic!("{package} is missing from Cargo.lock"));
    package_block
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("version = \"")
                .and_then(|v| v.strip_suffix('"'))
        })
        .unwrap_or_else(|| panic!("{package} has no version in Cargo.lock"))
        .to_owned()
}

fn npm_version(lock: &str, package: &str) -> String {
    let marker = format!("\"node_modules/{package}\": {{");
    let rest = lock
        .split_once(&marker)
        .unwrap_or_else(|| panic!("{package} is missing from package-lock.json"))
        .1;
    rest.lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("\"version\": \"")
                .and_then(|v| v.strip_suffix("\","))
        })
        .unwrap_or_else(|| panic!("{package} has no version in package-lock.json"))
        .to_owned()
}

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set");
    let root = Path::new(&manifest_dir)
        .parent()
        .expect("src-tauri has a parent directory");
    let cargo_lock = fs::read_to_string(root.join("Cargo.lock")).expect("read Cargo.lock");
    let npm_lock = fs::read_to_string(root.join("package-lock.json")).expect("read package-lock.json");

    println!(
        "cargo:rerun-if-changed={}",
        root.join("Cargo.lock").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        root.join("package-lock.json").display()
    );

    for (env_name, package) in [
        ("CRESCENDO_DEP_TAURI", "tauri"),
        ("CRESCENDO_DEP_TAURI_DIALOG", "tauri-plugin-dialog"),
        ("CRESCENDO_DEP_TAURI_OPENER", "tauri-plugin-opener"),
        ("CRESCENDO_DEP_GSTREAMER", "gstreamer"),
        ("CRESCENDO_DEP_TOKIO", "tokio"),
        ("CRESCENDO_DEP_RUSQLITE", "rusqlite"),
        ("CRESCENDO_DEP_LOFTY", "lofty"),
        ("CRESCENDO_DEP_IMAGE", "image"),
        ("CRESCENDO_DEP_UREQ", "ureq"),
        ("CRESCENDO_DEP_SEMVER", "semver"),
        ("CRESCENDO_DEP_SHA2", "sha2"),
        ("CRESCENDO_DEP_WALKDIR", "walkdir"),
        ("CRESCENDO_DEP_REGEX", "regex"),
        ("CRESCENDO_DEP_SERDE", "serde"),
    ] {
        println!(
            "cargo:rustc-env={env_name}={}",
            cargo_version(&cargo_lock, package)
        );
    }

    for (env_name, package) in [
        ("CRESCENDO_DEP_TAURI_API", "@tauri-apps/api"),
        ("CRESCENDO_DEP_SVELTE", "svelte"),
        ("CRESCENDO_DEP_SVELTEKIT", "@sveltejs/kit"),
        ("CRESCENDO_DEP_VITE", "vite"),
    ] {
        println!(
            "cargo:rustc-env={env_name}={}",
            npm_version(&npm_lock, package)
        );
    }

    tauri_build::build()
}
