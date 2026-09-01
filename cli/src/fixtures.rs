//! Checked-in evidence bundles for offline tests.
//!
//! The Midas family bundle at block 25,884,405 holds about 2,000 verbatim
//! responses (12 MB uncompressed), so it is committed as a tar.gz and
//! extracted once per build into `target/fixtures/`. The extraction is
//! atomic (extract into a temporary directory, then rename), so tests
//! running in parallel see either nothing or the complete bundle.

use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;

pub const MIDAS_FIXTURE_BLOCK: u64 = 25_884_405;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// The extracted Midas family bundle, `bundles/midas-run-25884405` in the
/// archive, extracted to `target/fixtures/midas-25884405/`.
pub fn midas_bundle() -> PathBuf {
    // One extraction per process; other test threads wait for it.
    static LOCK: Mutex<()> = Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let archive = workspace_root().join("cli/tests/fixtures/midas-25884405.tar.gz");
    let target = workspace_root().join("target/fixtures");
    let dir = target.join("midas-25884405");
    if dir.join("result.json").is_file() {
        return dir;
    }
    std::fs::create_dir_all(&target).expect("target/fixtures");
    let scratch = target.join(format!("midas-25884405.extract-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).expect("scratch directory");
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(&scratch)
        .status()
        .expect("tar is available");
    assert!(status.success(), "could not extract {}", archive.display());
    // The archive holds one top level directory.
    let inner = std::fs::read_dir(&scratch)
        .expect("scratch listing")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.is_dir())
        .expect("the archive holds one directory");
    match std::fs::rename(&inner, &dir) {
        Ok(()) => {}
        // Another test process extracted it first.
        Err(_) if dir.join("result.json").is_file() => {}
        Err(err) => panic!("could not move the fixture into place: {err}"),
    }
    let _ = std::fs::remove_dir_all(&scratch);
    dir
}
