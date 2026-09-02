//! The app's identity comes from `clatch.json`, at compile time.
//!
//! `id`, `connector.cli` and `name` used to be repeated as constants in `main.rs`, with a
//! test to catch the two drifting. Reading the manifest here and handing the values to the
//! crate as `env!()` makes the drift impossible rather than merely detected — and makes a
//! fork of this engine a manifest edit, not a Rust edit. That is the point: one engine,
//! rebranded per registry by the file the launcher already reads.

use std::path::Path;

fn main() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../clatch.json");
    println!("cargo:rerun-if-changed={}", manifest.display());

    let raw = std::fs::read_to_string(&manifest)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", manifest.display()));
    let m: serde_json::Value = serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("{} will not parse: {e}", manifest.display()));

    let field = |path: &[&str]| -> &str {
        let mut v = &m;
        for key in path {
            v = &v[*key];
        }
        v.as_str().unwrap_or_else(|| panic!("clatch.json is missing `{}`", path.join(".")))
    };
    println!("cargo:rustc-env=CLAPP_ID={}", field(&["id"]));
    println!("cargo:rustc-env=CLAPP_CLI={}", field(&["connector", "cli"]));
    println!("cargo:rustc-env=CLAPP_NAME={}", field(&["name"]));

    tauri_build::build()
}
