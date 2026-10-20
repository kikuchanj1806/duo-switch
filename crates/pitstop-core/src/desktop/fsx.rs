//! The careful file moves profiles need: writes that never overwrite,
//! copies that keep symlinks as symlinks, and names that never collide.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use super::io;
use crate::error::Result;

/// `20261020-091500-1a2b3c4d`: local time, then a random tail.
pub fn stamp() -> String {
    format!(
        "{}-{}",
        chrono::Local::now().format("%Y%m%d-%H%M%S"),
        hex8()
    )
}

/// Eight hex digits that differ from call to call.
pub fn hex8() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u64(SERIAL.fetch_add(1, Ordering::Relaxed));
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    hasher.write_u32(std::process::id());
    format!("{:08x}", hasher.finish() as u32)
}

/// Whether anything sits at `path`, a dangling symlink included.
pub fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

pub fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

/// A real directory, not a symlink to one.
pub fn is_real_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_dir())
}

/// A real file, not a symlink to one.
pub fn is_real_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_file())
}

pub fn mkdir_private(path: &Path) -> Result<()> {
    fs::DirBuilder::new()
        .mode(0o700)
        .create(path)
        .map_err(|e| io(path, e))
}

pub fn mkdirs_private(path: &Path) -> Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .map_err(|e| io(path, e))
}

fn temp_file(dir: &Path, prefix: &str, data: &[u8]) -> Result<PathBuf> {
    let temp = dir.join(format!("{prefix}{}", hex8()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)
        .map_err(|e| io(&temp, e))?;
    let written = file
        .write_all(data)
        .and_then(|()| file.sync_all())
        .map_err(|e| io(&temp, e));
    if let Err(e) = written {
        let _ = fs::remove_file(&temp);
        return Err(e);
    }
    Ok(temp)
}

/// Write `path` whole, or not at all, and never over something already
/// there: the data lands in a temporary file that is then hard-linked into
/// place, which fails when the name is taken.
pub fn exclusive_write(path: &Path, data: &[u8]) -> Result<()> {
    let dir = path.parent().expect("a file has a folder");
    let temp = temp_file(dir, ".claude-switch-", data)?;
    let linked = fs::hard_link(&temp, path).map_err(|e| io(path, e));
    let _ = fs::remove_file(&temp);
    linked
}

/// Replace `path` whole: the new data is written beside it, then renamed
/// over it.
pub fn replace_write(path: &Path, data: &[u8]) -> Result<()> {
    let dir = path.parent().expect("a file has a folder");
    let temp = temp_file(dir, ".meta-", data)?;
    fs::rename(&temp, path).map_err(|e| {
        let _ = fs::remove_file(&temp);
        io(path, e)
    })
}

/// Copy a file, a folder or a symlink; a symlink is copied as the link, never
/// followed. Files keep their times, as Python's `copy2` keeps them.
pub fn copy_entry(source: &Path, target: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(source).map_err(|e| io(source, e))?;
    if meta.file_type().is_symlink() {
        let to = fs::read_link(source).map_err(|e| io(source, e))?;
        std::os::unix::fs::symlink(&to, target).map_err(|e| io(target, e))
    } else if meta.is_dir() {
        fs::create_dir(target).map_err(|e| io(target, e))?;
        fs::set_permissions(target, meta.permissions()).map_err(|e| io(target, e))?;
        for entry in fs::read_dir(source).map_err(|e| io(source, e))? {
            let entry = entry.map_err(|e| io(source, e))?;
            copy_entry(&entry.path(), &target.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(source, target).map_err(|e| io(target, e))?;
        let times = fs::FileTimes::new()
            .set_accessed(meta.accessed().map_err(|e| io(source, e))?)
            .set_modified(meta.modified().map_err(|e| io(source, e))?);
        File::options()
            .write(true)
            .open(target)
            .and_then(|f| f.set_times(times))
            .map_err(|e| io(target, e))
    }
}

/// Every `local_*.json` directly in `dir`, files or not.
pub fn card_names(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| io(dir, e))? {
        let entry = entry.map_err(|e| io(dir, e))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("local_") && name.ends_with(".json") {
            out.push(entry.path());
        }
    }
    out.sort();
    Ok(out)
}

/// `path` with symlinks resolved as far as it exists; the missing rest is
/// kept as written.
pub fn resolve_lenient(path: &Path) -> PathBuf {
    let mut tail = Vec::new();
    let mut at = path.to_path_buf();
    loop {
        if let Ok(real) = fs::canonicalize(&at) {
            return tail
                .iter()
                .rev()
                .fold(real, |p: PathBuf, part| p.join(part));
        }
        match (at.parent(), at.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name.to_owned());
                at = parent.to_path_buf();
            }
            _ => return path.to_path_buf(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_exclusive_write_never_replaces_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("card.json");
        exclusive_write(&path, b"one").unwrap();
        assert!(exclusive_write(&path, b"two").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"one");
        // No temporary file left behind either way.
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn a_copy_keeps_symlinks_as_links() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("f"), "x").unwrap();
        std::os::unix::fs::symlink("/nowhere", src.join("link")).unwrap();
        copy_entry(&src, &dir.path().join("dst")).unwrap();
        assert_eq!(fs::read(dir.path().join("dst/f")).unwrap(), b"x");
        assert_eq!(
            fs::read_link(dir.path().join("dst/link")).unwrap(),
            Path::new("/nowhere")
        );
    }

    #[test]
    fn stamps_differ() {
        assert_ne!(stamp(), stamp());
    }
}
