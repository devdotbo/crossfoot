//! Embeds the resolved workspace package set into the binary.
//!
//! Bundles have to state which code produced them. Reading the workspace
//! Cargo.lock at run time would report whatever the lockfile happens to say then, which
//! is not necessarily what this binary was compiled from, so the list is
//! captured here at build time instead.

use std::fs;
use std::path::Path;

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set");
    let lock_path = Path::new(&manifest_dir).join("..").join("Cargo.lock");
    println!("cargo:rerun-if-changed={}", lock_path.display());

    let mut entries: Vec<String> = Vec::new();
    if let Ok(text) = fs::read_to_string(&lock_path) {
        let mut name: Option<String> = None;
        for line in text.lines() {
            let line = line.trim();
            if line == "[[package]]" {
                name = None;
            } else if let Some(rest) = line.strip_prefix("name = ") {
                name = Some(rest.trim_matches('"').to_string());
            } else if let Some(rest) = line.strip_prefix("version = ") {
                if let Some(name) = name.take() {
                    entries.push(format!("{name} {}", rest.trim_matches('"')));
                }
            }
        }
    }
    entries.sort();
    println!(
        "cargo:rustc-env=CROSSFOOT_LOCK_PACKAGES={}",
        entries.join(";")
    );
}
