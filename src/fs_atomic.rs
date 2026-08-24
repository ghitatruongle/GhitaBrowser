//! Crash-safe file replacement shared by the updater, extensions, installed
//! apps and Windows integration persistence.
//!
//! The naive write-temp-then-remove-then-rename dance leaves a window where a
//! crash destroys the destination. `MoveFileExW(MOVEFILE_REPLACE_EXISTING)`
//! replaces atomically on Windows; POSIX `rename` already replaces.

use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Unique temp sibling path so concurrent writers cannot clobber each other's
/// staging file (a fixed ".tmp" suffix let two managers rename foreign bytes).
fn temporary_path(path: &Path) -> std::path::PathBuf {
    let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .map(|name| format!("{}.{}.tmp", name.to_string_lossy(), unique))
        .unwrap_or_else(|| format!("ghita-{}.tmp", unique));
    path.with_file_name(name)
}

#[cfg(windows)]
fn replace_via_move_file(existing_temporary: &Path, path: &Path) -> io::Result<()> {
    use windows::core::HSTRING;
    use windows::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let to_wide =
        |value: &Path| -> Vec<u16> { HSTRING::from(value.as_os_str()).as_wide().to_vec() };
    let mut from = to_wide(existing_temporary);
    from.push(0);
    let mut to = to_wide(path);
    to.push(0);
    // SAFETY: both pointers are null-terminated wide strings owned for the
    // duration of the call; flags request atomic replace and write-through.
    unsafe {
        MoveFileExW(
            windows::core::PCWSTR(from.as_ptr()),
            windows::core::PCWSTR(to.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
        .map_err(|error| io::Error::other(error.to_string()))
    }
}

/// Write `bytes` to a unique temp sibling, flush it to disk, then atomically
/// replace `path`. On failure the temp file is removed and `path` is untouched.
pub fn atomic_write_bytes(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;

    if path.as_os_str().is_empty() {
        return Err(io::Error::other("destination path is empty"));
    }
    let temporary = temporary_path(path);

    let result = (|| {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);

        #[cfg(windows)]
        {
            if replace_via_move_file(&temporary, path).is_err() {
                // Fall back to POSIX-style rename for exotic filesystems
                // that reject MoveFileExW over an existing target.
                if path.exists() {
                    std::fs::remove_file(path)?;
                }
                std::fs::rename(&temporary, path)?;
            }
        }
        #[cfg(not(windows))]
        std::fs::rename(&temporary, path)?;

        Ok(())
    })();

    // On success the temp name has been consumed by the replace; on failure
    // it must not linger.
    let _ = std::fs::remove_file(&temporary);
    result
}

/// Serialize as pretty JSON and persist through [`atomic_write_bytes`].
pub fn atomic_write_json<T: serde::Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    atomic_write_bytes(path, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_replaces_and_leaves_no_temp_files() {
        let dir = std::env::temp_dir().join(format!("ghita_fs_atomic_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");

        atomic_write_bytes(&path, b"one").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"one");
        atomic_write_bytes(&path, b"two").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");

        let entries: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, vec!["state.json".to_string()]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn atomic_write_failure_preserves_destination() {
        let dir = std::env::temp_dir().join(format!("ghita_fs_atomic2_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("keep.json");
        atomic_write_bytes(&path, b"original").unwrap();

        // Occupy upcoming temp names with directories so the staged write
        // cannot be created. Candidate names are derived directly from the
        // counter (calling temporary_path would itself advance it). A wide
        // window absorbs concurrent tests incrementing the counter.
        let start = TEMP_COUNTER.load(Ordering::Relaxed);
        let mut blocked = Vec::new();
        for offset in 0u64..24 {
            let candidate = dir.join(format!("keep.json.{}.tmp", start + offset));
            if std::fs::create_dir_all(&candidate).is_ok() {
                blocked.push(candidate);
            }
        }
        assert!(atomic_write_bytes(&path, b"replacement").is_err());
        for candidate in &blocked {
            let _ = std::fs::remove_dir_all(candidate);
        }

        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
