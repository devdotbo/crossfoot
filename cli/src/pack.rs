//! `crossfoot bundle pack`: one deterministic archive per bundle, and the
//! matching unpack for `crossfoot verify <file.tar.gz>`.
//!
//! The archive is a gzip'd ustar stream with every field that would vary
//! between two runs fixed: entries sorted by path, mtime 0, uid and gid 0,
//! empty owner names, mode 0644 for files and 0755 for directories, a gzip
//! header with mtime 0 and no file name. Two packs of one bundle are
//! therefore byte for byte the same file, so the archive's sha256 is as
//! citable as the bundle root hash inside it.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use flate2::{Compression, GzBuilder};
use tar::{Archive, Builder, EntryType, Header};

use crate::cache::sha256_hex;

pub struct Packed {
    pub archive: PathBuf,
    pub archive_sha256: String,
    /// From the bundle's bundle.sha256, when it is sealed.
    pub root_hash: Option<String>,
    pub files: usize,
}

/// Every path under `dir`, directories and files, relative to `dir`, as
/// (relative path, is_dir), sorted by path so parents precede children.
fn listing(dir: &Path) -> Result<Vec<(String, bool)>, String> {
    fn walk(root: &Path, current: &Path, out: &mut Vec<(String, bool)>) -> Result<(), String> {
        for entry in fs::read_dir(current)
            .map_err(|err| format!("could not read {}: {err}", current.display()))?
        {
            let entry =
                entry.map_err(|err| format!("could not read {}: {err}", current.display()))?;
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .map_err(|_| format!("{} is outside {}", path.display(), root.display()))?
                .to_string_lossy()
                .replace('\\', "/");
            if path.is_dir() {
                out.push((relative, true));
                walk(root, &path, out)?;
            } else if path.is_file() {
                out.push((relative, false));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out)?;
    out.sort();
    Ok(out)
}

/// Writes `<out>` as the deterministic archive of `bundle_dir`. The archive
/// holds one top-level directory named after the bundle.
pub fn pack(bundle_dir: &Path, out: &Path) -> Result<Packed, String> {
    let bundle_dir = bundle_dir
        .canonicalize()
        .map_err(|err| format!("{} is not readable: {err}", bundle_dir.display()))?;
    if !bundle_dir.join("manifest.json").is_file() {
        return Err(format!(
            "{} holds no manifest.json, so it is not a bundle",
            bundle_dir.display()
        ));
    }
    let name = bundle_dir
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("the bundle directory has no name")?
        .to_string();

    let mut entries = vec![(String::new(), true)];
    entries.extend(listing(&bundle_dir)?);

    let file = fs::File::create(out)
        .map_err(|err| format!("could not create {}: {err}", out.display()))?;
    let gz = GzBuilder::new()
        .mtime(0)
        .operating_system(255)
        .write(file, Compression::default());
    let mut builder = Builder::new(gz);
    let mut files = 0usize;
    for (relative, is_dir) in &entries {
        let archived = if relative.is_empty() {
            format!("{name}/")
        } else if *is_dir {
            format!("{name}/{relative}/")
        } else {
            format!("{name}/{relative}")
        };
        let mut header = Header::new_ustar();
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0);
        header.set_username("").map_err(|err| err.to_string())?;
        header.set_groupname("").map_err(|err| err.to_string())?;
        if *is_dir {
            header.set_entry_type(EntryType::Directory);
            header.set_mode(0o755);
            header.set_size(0);
            builder
                .append_data(&mut header, &archived, std::io::empty())
                .map_err(|err| format!("could not add {archived}: {err}"))?;
        } else {
            let bytes = fs::read(bundle_dir.join(relative))
                .map_err(|err| format!("could not read {relative}: {err}"))?;
            header.set_entry_type(EntryType::Regular);
            header.set_mode(0o644);
            header.set_size(bytes.len() as u64);
            builder
                .append_data(&mut header, &archived, &bytes[..])
                .map_err(|err| format!("could not add {archived}: {err}"))?;
            files += 1;
        }
    }
    let gz = builder
        .into_inner()
        .map_err(|err| format!("could not finish the archive: {err}"))?;
    gz.finish()
        .map_err(|err| format!("could not finish the archive: {err}"))?;

    let archive_sha256 = sha256_hex(
        &fs::read(out).map_err(|err| format!("could not read back {}: {err}", out.display()))?,
    );
    let root_hash = fs::read_to_string(bundle_dir.join("bundle.sha256"))
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|hash| hash.len() == 64);
    Ok(Packed {
        archive: out.to_path_buf(),
        archive_sha256,
        root_hash,
        files,
    })
}

/// Unpacks a bundle archive into `into` and returns the one top-level
/// directory it holds. Entries that would land outside `into` are refused
/// by the tar reader.
pub fn unpack(archive: &Path, into: &Path) -> Result<PathBuf, String> {
    fs::create_dir_all(into)
        .map_err(|err| format!("could not create {}: {err}", into.display()))?;
    let file = fs::File::open(archive)
        .map_err(|err| format!("could not open {}: {err}", archive.display()))?;
    let mut tar = Archive::new(GzDecoder::new(file));
    tar.unpack(into)
        .map_err(|err| format!("could not unpack {}: {err}", archive.display()))?;
    let mut dirs: Vec<PathBuf> = fs::read_dir(into)
        .map_err(|err| format!("could not read {}: {err}", into.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    match dirs.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err(format!("{} holds no directory", archive.display())),
        many => Err(format!(
            "{} holds {} top-level directories, a bundle archive holds one",
            archive.display(),
            many.len()
        )),
    }
}

/// True when the path names a gzip'd tar file rather than a directory: it
/// is a regular file that starts with the gzip magic bytes.
pub fn is_archive(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    let mut magic = [0u8; 2];
    fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut magic))
        .map(|_| magic == [0x1f, 0x8b])
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("svzchf-demo-24570000-25853000")
    }

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("crossfoot-pack-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Two packs of one bundle are one file, and the archive round-trips
    /// to the same files with the same bytes.
    #[test]
    fn pack_twice_is_byte_identical() {
        let dir = temp("twice");
        let first = pack(&fixture(), &dir.join("a.tar.gz")).unwrap();
        let second = pack(&fixture(), &dir.join("b.tar.gz")).unwrap();
        let a = fs::read(&first.archive).unwrap();
        let b = fs::read(&second.archive).unwrap();
        assert_eq!(a, b, "the two archives differ");
        assert_eq!(first.archive_sha256, second.archive_sha256);
        assert_eq!(first.archive_sha256, sha256_hex(&a));
        assert_eq!(
            first.files, 37,
            "32 raw files plus the five top-level files"
        );
        assert_eq!(
            first.root_hash.as_deref(),
            Some(
                fs::read_to_string(fixture().join("bundle.sha256"))
                    .unwrap()
                    .trim()
            )
        );
        assert!(is_archive(&first.archive));
        assert!(!is_archive(&fixture()));

        let unpacked = unpack(&first.archive, &dir.join("out")).unwrap();
        assert_eq!(
            unpacked.file_name().unwrap().to_str().unwrap(),
            "svzchf-demo-24570000-25853000"
        );
        for (relative, is_dir) in listing(&fixture()).unwrap() {
            if !is_dir {
                assert_eq!(
                    fs::read(fixture().join(&relative)).unwrap(),
                    fs::read(unpacked.join(&relative)).unwrap(),
                    "{relative} differs after the round trip"
                );
            }
        }
        // Every entry carries the fixed metadata.
        let file = fs::File::open(&first.archive).unwrap();
        let mut tar = Archive::new(GzDecoder::new(file));
        let mut previous = String::new();
        for entry in tar.entries().unwrap() {
            let entry = entry.unwrap();
            let header = entry.header();
            assert_eq!(header.mtime().unwrap(), 0);
            assert_eq!(header.uid().unwrap(), 0);
            assert_eq!(header.gid().unwrap(), 0);
            let path = entry.path().unwrap().to_string_lossy().to_string();
            assert!(
                path > previous,
                "entries are sorted: {previous} then {path}"
            );
            previous = path;
        }
    }
}
