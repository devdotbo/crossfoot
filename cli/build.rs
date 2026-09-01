//! Embeds the resolved workspace package set into the binary.
//!
//! Bundles have to state which code produced them. Reading the workspace
//! Cargo.lock at run time would report whatever the lockfile happens to say then, which
//! is not necessarily what this binary was compiled from, so the list is
//! captured here at build time instead.

use std::fs;
use std::path::Path;
use std::process::Command;

/// The git commit and dirty state of the tree the binary was built from.
/// This is the binary's identity, distinct from the run-time repository
/// state meta.json also records: a verifier compares its own identity with
/// the one in a bundle's manifest. A missing git binary or no repository
/// gives "unknown" rather than a guess.
fn git_identity(repo_dir: &Path) -> (String, String) {
    let run = |args: &[&str]| -> Option<String> {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo_dir)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    };
    let commit = run(&["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".to_string());
    let dirty = match run(&["status", "--porcelain", "--untracked-files=no"]) {
        Some(status) => (!status.trim().is_empty()).to_string(),
        None => "unknown".to_string(),
    };
    (commit, dirty)
}

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set");
    let repo_dir = Path::new(&manifest_dir).join("..");
    let lock_path = repo_dir.join("Cargo.lock");
    println!("cargo:rerun-if-changed={}", lock_path.display());

    // Rebuild when HEAD moves. In a worktree .git is a file pointing at the
    // real directory, so both shapes are watched; the tracked source files
    // change the build anyway.
    for probe in [".git/HEAD", ".git/index", ".git"] {
        let path = repo_dir.join(probe);
        if path.exists() {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
    let (commit, dirty) = git_identity(&repo_dir);
    println!("cargo:rustc-env=CROSSFOOT_GIT_COMMIT={commit}");
    println!("cargo:rustc-env=CROSSFOOT_GIT_DIRTY={dirty}");

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
