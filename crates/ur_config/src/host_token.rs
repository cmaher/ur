//! Host credential for ur-server's host-facing gRPC listener.
//!
//! `ur start` ensures a random token exists at `$UR_CONFIG/host-token` (mode
//! 0600) and hands it to ur-server through the compose environment
//! ([`UR_HOST_TOKEN_ENV`]). Every host-side client reads the file and sends
//! the token in the [`HOST_TOKEN_HEADER`] metadata header. Container workers
//! never see the file: no worker mount covers the config dir root.

use std::fs;
use std::io::{ErrorKind, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// gRPC metadata header carrying the host token on host-listener requests.
pub const HOST_TOKEN_HEADER: &str = "ur-host-token";

/// Environment variable through which ur-server receives the host token.
pub const UR_HOST_TOKEN_ENV: &str = "UR_HOST_TOKEN";

/// Filename of the host token, directly under the config dir.
pub const HOST_TOKEN_FILE: &str = "host-token";

/// Random bytes in a generated token (hex-encoded on disk).
const HOST_TOKEN_BYTES: usize = 32;

/// Shortest token accepted from the file or the environment.
pub const MIN_HOST_TOKEN_LEN: usize = 32;

/// Path of the host token file for a config dir.
pub fn host_token_path(config_dir: &Path) -> PathBuf {
    config_dir.join(HOST_TOKEN_FILE)
}

/// Validate a token value: at least [`MIN_HOST_TOKEN_LEN`] visible ASCII
/// characters, so it is usable as a gRPC metadata value.
pub fn validate_host_token(token: &str) -> Result<()> {
    if token.len() < MIN_HOST_TOKEN_LEN {
        bail!("host token is shorter than {MIN_HOST_TOKEN_LEN} characters");
    }
    if !token.bytes().all(|b| b.is_ascii_graphic()) {
        bail!("host token contains characters other than visible ASCII");
    }
    Ok(())
}

/// Read the host token from `$config_dir/host-token`.
///
/// Errors name the file, so a CLI that cannot authenticate says why.
pub fn read_host_token(config_dir: &Path) -> Result<String> {
    let path = host_token_path(config_dir);
    let raw = fs::read_to_string(&path).with_context(|| {
        format!(
            "cannot read host token file {} (run 'ur server start' to create it)",
            path.display()
        )
    })?;
    let token = raw.trim().to_owned();
    validate_host_token(&token).with_context(|| {
        format!(
            "invalid host token file {} (delete it and run 'ur server restart')",
            path.display()
        )
    })?;
    Ok(token)
}

/// Return the host token, creating `$config_dir/host-token` (mode 0600, 32
/// bytes from the OS RNG, hex-encoded) when absent.
///
/// Creation is atomic: the token is written to a private temp file and
/// hard-linked into place, so concurrent callers all end up with the token
/// of whichever link won. An existing file's permissions are tightened to
/// 0600.
pub fn ensure_host_token(config_dir: &Path) -> Result<String> {
    let path = host_token_path(config_dir);
    if !path.exists() {
        create_host_token_file(config_dir, &path)?;
    }
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("failed to set permissions on {}", path.display()))?;
    read_host_token(config_dir)
}

fn create_host_token_file(config_dir: &Path, path: &Path) -> Result<()> {
    fs::create_dir_all(config_dir)
        .with_context(|| format!("failed to create {}", config_dir.display()))?;
    let mut bytes = [0u8; HOST_TOKEN_BYTES];
    getrandom::fill(&mut bytes)
        .map_err(|e| anyhow::anyhow!("failed to read random bytes from the OS: {e}"))?;
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();

    let tmp = config_dir.join(format!(".{HOST_TOKEN_FILE}.{}.tmp", std::process::id()));
    let write_tmp = || -> std::io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(token.as_bytes())?;
        file.sync_all()
    };
    let result = write_tmp().and_then(|()| match fs::hard_link(&tmp, path) {
        Err(e) if e.kind() == ErrorKind::AlreadyExists => Ok(()),
        other => other,
    });
    let _ = fs::remove_file(&tmp);
    result.with_context(|| format!("failed to create host token file {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_creates_private_token_and_reuses_it() {
        let dir = tempfile::tempdir().unwrap();
        let first = ensure_host_token(dir.path()).unwrap();
        assert_eq!(first.len(), HOST_TOKEN_BYTES * 2);
        assert!(first.bytes().all(|b| b.is_ascii_hexdigit()));
        let mode = fs::metadata(host_token_path(dir.path()))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);

        let second = ensure_host_token(dir.path()).unwrap();
        assert_eq!(first, second);
        assert_eq!(read_host_token(dir.path()).unwrap(), first);
        let leftovers: Vec<_> = fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(leftovers.len(), 1, "temp file left behind");
    }

    #[test]
    fn ensure_tightens_loose_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = host_token_path(dir.path());
        fs::write(&path, "a".repeat(40)).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(ensure_host_token(dir.path()).unwrap(), "a".repeat(40));
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn read_missing_file_names_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let err = format!("{:#}", read_host_token(dir.path()).unwrap_err());
        assert!(err.contains(&host_token_path(dir.path()).display().to_string()));
    }

    #[test]
    fn read_rejects_short_or_unprintable_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let path = host_token_path(dir.path());
        fs::write(&path, "short").unwrap();
        assert!(read_host_token(dir.path()).is_err());
        fs::write(&path, format!("{}\u{7f}", "a".repeat(40))).unwrap();
        assert!(read_host_token(dir.path()).is_err());
    }
}
