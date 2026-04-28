use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Exact content of the hostexec script shim.
///
/// Each declared hostexec script is bind-mounted over its container path with
/// this shim as the content. When the worker executes the script, the shim
/// invokes `workertools host-exec --script <resolved-host-path> "$@"`, which
/// routes the call back to the server over gRPC.
pub const SHIM_CONTENT: &str =
    "#!/bin/sh\nexec workertools host-exec --script \"$(readlink -f \"$0\")\" \"$@\"\n";

/// Materializes the hostexec script shim at `$URCONFIG/hostexec/script-shim.sh`.
///
/// The write is atomic (temp file + rename) and idempotent: if the file already
/// exists with the correct content, no write occurs.
///
/// Returns the resolved host path to the shim so callers (e.g. `RunOptsBuilder`)
/// can use it as the bind-mount source.
pub fn materialize_shim(config_dir: &Path) -> anyhow::Result<PathBuf> {
    let hostexec_dir = config_dir.join(ur_config::HOSTEXEC_DIR);
    std::fs::create_dir_all(&hostexec_dir)?;

    let shim_path = hostexec_dir.join("script-shim.sh");
    write_shim_if_needed(&shim_path)?;
    Ok(shim_path)
}

/// Write the shim to `path` atomically if the current content differs.
fn write_shim_if_needed(path: &Path) -> anyhow::Result<()> {
    // Read existing content to check for idempotency.
    if let Ok(existing) = std::fs::read_to_string(path) {
        if existing == SHIM_CONTENT {
            return Ok(());
        }
    }

    // Write to a sibling temp file then rename into place.
    let dir = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("shim path has no parent directory"))?;
    let tmp_path = dir.join(".script-shim.sh.tmp");

    std::fs::write(&tmp_path, SHIM_CONTENT)?;
    std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o755))?;
    std::fs::rename(&tmp_path, path)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn shim_path(dir: &TempDir) -> PathBuf {
        dir.path()
            .join(ur_config::HOSTEXEC_DIR)
            .join("script-shim.sh")
    }

    #[test]
    fn fresh_write_creates_shim_with_correct_content_and_mode() {
        let tmp = TempDir::new().unwrap();
        let path = materialize_shim(tmp.path()).unwrap();

        assert_eq!(path, shim_path(&tmp));
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, SHIM_CONTENT);

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        // Verify owner-execute bit (and group/other execute bits) are set.
        assert_eq!(mode & 0o111, 0o111, "shim must be executable");
        assert_eq!(mode & 0o755, 0o755, "shim must have mode 0755");
    }

    #[test]
    fn idempotent_rewrite_does_not_change_file() {
        let tmp = TempDir::new().unwrap();
        let path = materialize_shim(tmp.path()).unwrap();

        let before_meta = std::fs::metadata(&path).unwrap();
        let before_mtime = before_meta.modified().unwrap();

        // Sleep briefly to ensure mtime would differ if a write happened.
        std::thread::sleep(std::time::Duration::from_millis(20));

        materialize_shim(tmp.path()).unwrap();

        let after_mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(
            before_mtime, after_mtime,
            "mtime must not change on idempotent call"
        );
    }

    #[test]
    fn content_drift_is_replaced_atomically() {
        let tmp = TempDir::new().unwrap();
        let path = materialize_shim(tmp.path()).unwrap();

        // Overwrite with stale content.
        std::fs::write(&path, "#!/bin/sh\necho stale\n").unwrap();
        // Strip execute bit to verify it is restored.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        materialize_shim(tmp.path()).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, SHIM_CONTENT, "drifted content must be replaced");

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o755, 0o755, "mode must be restored to 0755");
    }
}
