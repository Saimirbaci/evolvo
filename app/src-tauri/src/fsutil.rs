//! Small filesystem helpers shared across `store.rs`, `stages.rs`, and
//! `runner.rs`. Kept dependency-free (no `Store`/`StoreError` coupling) so
//! any module can pull these in without creating a cycle.

use std::fs;
use std::io;
use std::path::Path;

/// Write `bytes` to `path` atomically: same-directory `.tmp` file + rename.
/// A crash mid-`fs::write` truncates the record, and a truncated record is
/// silently skipped by readers that expect well-formed JSON — the
/// job/feedback/plan "vanishes". Rename within one directory is atomic on
/// every platform we target. The `.tmp` sibling never matches the `.json`
/// extension filter used by directory listers, so a crash between write and
/// rename leaves only harmless debris (swept on next `init_workspace`).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no filename"))?;
    let tmp = path.with_file_name(format!("{file_name}.tmp"));
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Create `path` (and parents) with owner-only permissions (`0700`) on
/// Unix. Lineage workspaces contain source-repo worktrees, API-key-bearing
/// env files, and agent transcripts that may quote file contents — no
/// reason for other local users to have read access. Best-effort on the
/// permission bits: directory creation itself still fails loudly (`?`),
/// but a platform that doesn't support Unix permissions (or a filesystem
/// that rejects `chmod`) falls back to whatever `create_dir_all` produced
/// rather than blocking startup.
#[cfg(unix)]
pub fn create_dir_all_private(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
}

#[cfg(not(unix))]
pub fn create_dir_all_private(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)
}

/// Best-effort tighten of an already-existing directory to `0700`. Used at
/// startup on top-level workspace directories that may have been created
/// by an older Evolvo build (before this hardening landed) with default
/// permissions. Never fails startup — a filesystem that doesn't support
/// Unix permission bits (or a `chmod` that's refused for any reason)
/// silently leaves the directory as-is.
#[cfg(unix)]
pub fn tighten_permissions_best_effort(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = fs::metadata(path) {
        let mut perms = meta.permissions();
        perms.set_mode(0o700);
        let _ = fs::set_permissions(path, perms);
    }
}

#[cfg(not(unix))]
pub fn tighten_permissions_best_effort(_path: &Path) {}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::tempdir;

    #[test]
    fn create_dir_all_private_sets_0700() {
        let temp = tempdir().unwrap();
        let nested = temp.path().join("a").join("b");
        create_dir_all_private(&nested).unwrap();
        let mode = fs::metadata(&nested).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn tighten_permissions_best_effort_sets_0700() {
        let temp = tempdir().unwrap();
        let dir = temp.path().join("loose");
        fs::create_dir_all(&dir).unwrap();
        let mut perms = fs::metadata(&dir).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&dir, perms).unwrap();

        tighten_permissions_best_effort(&dir);
        let mode = fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn write_atomic_round_trips() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("f.json");
        write_atomic(&path, b"{\"a\":1}").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"{\"a\":1}");
        // No stray .tmp left behind.
        assert!(!temp.path().join("f.json.tmp").exists());
    }
}
