//! Small helpers: UTC timestamps, hex block numbers, git and package
//! provenance.

use std::process::Command;

use chrono::{SecondsFormat, Utc};
use serde::Serialize;

pub fn now_utc() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Compact form used in bundle directory names, for example 20260828T134501Z.
pub fn now_stamp() -> String {
    Utc::now().format("%Y%m%dT%H%M%SZ").to_string()
}

pub fn unix_to_utc(seconds: i64) -> Option<String> {
    chrono::DateTime::from_timestamp(seconds, 0)
        .map(|dt| dt.to_rfc3339_opts(SecondsFormat::Secs, true))
}

pub fn block_hex(block: u64) -> String {
    format!("0x{block:x}")
}

pub fn parse_hex_u64(value: &str) -> Option<u64> {
    u64::from_str_radix(value.strip_prefix("0x").unwrap_or(value), 16).ok()
}

/// Turn a read label into a filesystem safe, stable slug for raw file names.
pub fn slug(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    let mut last_was_dash = false;
    for ch in label.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash && !out.is_empty() {
            out.push('-');
            last_was_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

#[derive(Debug, Clone, Serialize)]
pub struct GitProvenance {
    pub describe: Option<String>,
    pub commit: Option<String>,
    pub dirty: Option<bool>,
}

/// Records the state of the repository that produced a bundle. Every field is
/// optional: a missing git binary or a repository without commits is recorded
/// as null rather than guessed at.
pub fn git_provenance(repo_dir: &std::path::Path) -> GitProvenance {
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
    let commit = run(&["rev-parse", "HEAD"]);
    let describe = run(&["describe", "--always", "--dirty", "--tags"]);
    let dirty = run(&["status", "--porcelain"]).map(|status| !status.trim().is_empty());
    GitProvenance {
        describe,
        commit,
        dirty,
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PackageVersion {
    pub name: String,
    pub version: String,
}

/// The resolved package set of the workspace, captured from the workspace
/// Cargo.lock at build time by build.rs. This is the exact dependency
/// set the binary was compiled from, not whatever the lockfile says at run
/// time.
pub fn workspace_packages() -> Vec<PackageVersion> {
    let raw = env!("CROSSFOOT_LOCK_PACKAGES");
    raw.split(';')
        .filter(|entry| !entry.is_empty())
        .filter_map(|entry| {
            let (name, version) = entry.split_once(' ')?;
            Some(PackageVersion {
                name: name.to_string(),
                version: version.to_string(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_hex_round_trips() {
        assert_eq!(block_hex(24_570_000), "0x176e890");
        assert_eq!(parse_hex_u64("0x176e890"), Some(24_570_000));
        assert_eq!(parse_hex_u64("176e890"), Some(24_570_000));
    }

    #[test]
    fn slugs_are_filesystem_safe() {
        assert_eq!(slug("vault.convertToAssets(1e18)"), "vault-converttoassets-1e18");
        assert_eq!(slug("module.INTEREST_DELAY()"), "module-interest-delay");
        assert_eq!(slug("eth_getLogs 0x1..0x2"), "eth-getlogs-0x1-0x2");
    }

    #[test]
    fn workspace_packages_include_this_crate() {
        let packages = workspace_packages();
        assert!(
            packages.iter().any(|p| p.name == "crossfoot"),
            "expected crossfoot in the embedded lockfile package list"
        );
        assert!(
            packages.iter().any(|p| p.name == "actus-pam"),
            "expected actus-pam in the embedded lockfile package list"
        );
    }
}
