use crate::HostExecError;

/// The only hostexec command allowed to run with a `/brain` working directory: brain
/// skills commit to the brain's own git repository from inside it.
pub const BRAIN_HOSTEXEC_COMMAND: &str = "git";

/// Whether `container_dir` is the `/brain` mount or a directory under it.
pub fn is_brain_dir(container_dir: &str) -> bool {
    container_dir
        .strip_prefix(ur_config::BRAIN_CONTAINER_PATH)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// Map a `/brain[/...]` container directory to the same place under the host brain dir.
///
/// Builderd passes absolute paths through unchanged. `..` segments are rejected so the
/// mapping can never leave the brain.
#[allow(clippy::result_large_err)]
pub fn map_brain_working_dir_impl(
    container_dir: &str,
    brain_host_path: &std::path::Path,
) -> Result<String, HostExecError> {
    let invalid = |reason: &str| HostExecError::InvalidWorkingDir {
        path: container_dir.to_string(),
        reason: reason.into(),
    };
    let rest = container_dir
        .strip_prefix(ur_config::BRAIN_CONTAINER_PATH)
        .filter(|rest| rest.is_empty() || rest.starts_with('/'))
        .ok_or_else(|| invalid("working_dir must start with /brain"))?;
    let rel = rest.trim_start_matches('/');
    if rel.split('/').any(|seg| seg == "..") {
        return Err(invalid("working_dir must not contain '..'"));
    }
    let mapped = if rel.is_empty() {
        brain_host_path.to_path_buf()
    } else {
        brain_host_path.join(rel)
    };
    Ok(mapped.to_string_lossy().into_owned())
}

/// Validate a container-relative script path and return the portion after `/workspace/`.
///
/// Returns `Err` with a human-readable message if the path does not start with
/// `/workspace/`, is empty after stripping the prefix, or contains `..` segments.
pub fn validate_script_path(script_path: &str) -> Result<&str, String> {
    let rel = script_path
        .strip_prefix("/workspace/")
        .ok_or_else(|| format!("script_path must start with /workspace/: {script_path}"))?;

    if rel.is_empty() {
        return Err(format!(
            "script_path must not be /workspace/ alone: {script_path}"
        ));
    }

    // Reject path traversal
    for segment in rel.split('/') {
        if segment == ".." || segment.is_empty() {
            return Err(format!(
                "script_path contains invalid segment: {script_path}"
            ));
        }
    }

    Ok(rel)
}

/// Map a container-absolute script path (`/workspace/<rel>`) to a `%WORKSPACE%` template.
///
/// Delegates to `map_working_dir_impl` — the path prefix logic is identical.
/// Pool mode produces `%WORKSPACE%/<slot_rel>/<rel>`; workspace mode produces `%WORKSPACE%/<rel>`.
#[allow(clippy::result_large_err)]
pub fn map_script_path_impl(
    script_path: &str,
    slot_path: Option<&std::path::Path>,
    host_workspace: &std::path::Path,
) -> Result<String, HostExecError> {
    map_working_dir_impl(script_path, slot_path, host_workspace)
}

/// Map container CWD to a `%WORKSPACE%` template path for builderd (implementation).
///
/// Extracted as a free function for testability.
#[allow(clippy::result_large_err)]
pub fn map_working_dir_impl(
    container_dir: &str,
    slot_path: Option<&std::path::Path>,
    host_workspace: &std::path::Path,
) -> Result<String, HostExecError> {
    let Some(suffix) = container_dir.strip_prefix("/workspace") else {
        return Err(HostExecError::InvalidWorkingDir {
            path: container_dir.to_string(),
            reason: "working_dir must start with /workspace".into(),
        });
    };

    if !suffix.is_empty() && !suffix.starts_with('/') {
        return Err(HostExecError::InvalidWorkingDir {
            path: container_dir.to_string(),
            reason: "invalid working_dir".into(),
        });
    }

    // For pool mounts, slot_path is under host_workspace — use %WORKSPACE%/<relative> template.
    // For workspace mounts (-w), slot_path is an arbitrary host path not under host_workspace —
    // use the absolute slot_path directly (builderd passes through absolute paths unchanged).
    if let Some(sp) = slot_path {
        let host_ws = host_workspace.to_string_lossy();
        let sp_str = sp.to_string_lossy();
        if let Some(rel) = sp_str.strip_prefix(host_ws.as_ref()) {
            // Pool mount: slot is under host_workspace
            let slot_relative = rel.trim_start_matches('/');
            let mut result = "%WORKSPACE%".to_string();
            if !slot_relative.is_empty() {
                result.push('/');
                result.push_str(slot_relative);
            }
            if !suffix.is_empty() {
                result.push_str(suffix);
            }
            Ok(result)
        } else {
            // Workspace mount: slot is an absolute path outside host_workspace
            let mut result = sp_str.into_owned();
            if !suffix.is_empty() {
                result.push_str(suffix);
            }
            Ok(result)
        }
    } else {
        // No slot_path: direct workspace mount
        let mut result = "%WORKSPACE%".to_string();
        if !suffix.is_empty() {
            result.push_str(suffix);
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const HOST_WS: &str = "/home/user/.ur/workspace";

    #[test]
    fn test_map_working_dir_workspace_mount() {
        let result = map_working_dir_impl("/workspace", None, Path::new(HOST_WS)).unwrap();
        assert_eq!(result, "%WORKSPACE%");
    }

    #[test]
    fn test_map_working_dir_workspace_mount_subdir() {
        let result = map_working_dir_impl("/workspace/src/main", None, Path::new(HOST_WS)).unwrap();
        assert_eq!(result, "%WORKSPACE%/src/main");
    }

    #[test]
    fn test_map_working_dir_pool_mount() {
        let slot = std::path::PathBuf::from("/home/user/.ur/workspace/pool/proj/0");
        let result = map_working_dir_impl("/workspace", Some(&slot), Path::new(HOST_WS)).unwrap();
        assert_eq!(result, "%WORKSPACE%/pool/proj/0");
    }

    #[test]
    fn test_map_working_dir_pool_mount_subdir() {
        let result = map_working_dir_impl(
            "/workspace/src/main",
            Some(&std::path::PathBuf::from(
                "/home/user/.ur/workspace/pool/proj/0",
            )),
            Path::new(HOST_WS),
        )
        .unwrap();
        assert_eq!(result, "%WORKSPACE%/pool/proj/0/src/main");
    }

    #[test]
    fn test_map_working_dir_workspace_mount_with_slot_path() {
        // -w mount: slot_path is an arbitrary host dir, not under host_workspace
        let slot = std::path::PathBuf::from("/Users/foo/myproject");
        let result = map_working_dir_impl("/workspace", Some(&slot), Path::new(HOST_WS)).unwrap();
        assert_eq!(result, "/Users/foo/myproject");
    }

    #[test]
    fn test_map_working_dir_workspace_mount_with_slot_path_subdir() {
        let slot = std::path::PathBuf::from("/Users/foo/myproject");
        let result =
            map_working_dir_impl("/workspace/src", Some(&slot), Path::new(HOST_WS)).unwrap();
        assert_eq!(result, "/Users/foo/myproject/src");
    }

    #[test]
    fn is_brain_dir_matches_mount_and_children_only() {
        assert!(is_brain_dir("/brain"));
        assert!(is_brain_dir("/brain/"));
        assert!(is_brain_dir("/brain/working/x"));
        assert!(!is_brain_dir("/brainfoo"));
        assert!(!is_brain_dir("/workspace/brain"));
        assert!(!is_brain_dir("/tmp"));
    }

    #[test]
    fn map_brain_working_dir_maps_under_host_brain() {
        let brain = Path::new("/host/config/projects/p/brain");
        assert_eq!(
            map_brain_working_dir_impl("/brain", brain).unwrap(),
            "/host/config/projects/p/brain"
        );
        assert_eq!(
            map_brain_working_dir_impl("/brain/working/s", brain).unwrap(),
            "/host/config/projects/p/brain/working/s"
        );
    }

    #[test]
    fn map_brain_working_dir_rejects_escape_and_other_paths() {
        let brain = Path::new("/host/brain");
        assert!(map_brain_working_dir_impl("/brain/../etc", brain).is_err());
        assert!(map_brain_working_dir_impl("/brain/a/../../x", brain).is_err());
        assert!(map_brain_working_dir_impl("/brainfoo", brain).is_err());
        assert!(map_brain_working_dir_impl("/workspace", brain).is_err());
    }

    #[test]
    fn test_map_working_dir_rejects_invalid() {
        assert!(map_working_dir_impl("/tmp", None, Path::new(HOST_WS)).is_err());
        assert!(map_working_dir_impl("/workspacefoo", None, Path::new(HOST_WS)).is_err());
    }

    #[test]
    fn test_map_working_dir_rejects_invalid_with_pool_slot() {
        let slot = std::path::PathBuf::from("/home/user/.ur/workspace/pool/proj/0");
        assert!(map_working_dir_impl("/tmp", Some(&slot), Path::new(HOST_WS)).is_err());
        assert!(map_working_dir_impl("/workspacefoo", Some(&slot), Path::new(HOST_WS)).is_err());
    }

    #[test]
    fn test_map_working_dir_rejects_invalid_with_workspace_slot() {
        let slot = std::path::PathBuf::from("/Users/foo/myproject");
        assert!(map_working_dir_impl("/other", Some(&slot), Path::new(HOST_WS)).is_err());
    }

    #[test]
    fn test_map_working_dir_pool_slot_is_host_workspace_root() {
        // Slot path is exactly the host_workspace directory (no relative suffix)
        let slot = std::path::PathBuf::from(HOST_WS);
        let result = map_working_dir_impl("/workspace", Some(&slot), Path::new(HOST_WS)).unwrap();
        assert_eq!(result, "%WORKSPACE%");
    }

    #[test]
    fn test_map_working_dir_pool_slot_is_host_workspace_root_subdir() {
        let slot = std::path::PathBuf::from(HOST_WS);
        let result =
            map_working_dir_impl("/workspace/src", Some(&slot), Path::new(HOST_WS)).unwrap();
        assert_eq!(result, "%WORKSPACE%/src");
    }

    #[test]
    fn test_map_working_dir_pool_mount_deeply_nested_subdir() {
        let slot = std::path::PathBuf::from("/home/user/.ur/workspace/pool/proj/0");
        let result = map_working_dir_impl(
            "/workspace/src/lib/module/file",
            Some(&slot),
            Path::new(HOST_WS),
        )
        .unwrap();
        assert_eq!(result, "%WORKSPACE%/pool/proj/0/src/lib/module/file");
    }

    #[test]
    fn test_map_working_dir_workspace_mount_deeply_nested_subdir() {
        let result = map_working_dir_impl("/workspace/a/b/c/d", None, Path::new(HOST_WS)).unwrap();
        assert_eq!(result, "%WORKSPACE%/a/b/c/d");
    }

    #[test]
    fn test_map_working_dir_workspace_slot_deeply_nested_subdir() {
        let slot = std::path::PathBuf::from("/Users/foo/myproject");
        let result =
            map_working_dir_impl("/workspace/a/b/c/d", Some(&slot), Path::new(HOST_WS)).unwrap();
        assert_eq!(result, "/Users/foo/myproject/a/b/c/d");
    }

    #[test]
    fn test_map_working_dir_error_message_contains_path() {
        let err = map_working_dir_impl("/tmp/foo", None, Path::new(HOST_WS)).unwrap_err();
        match err {
            HostExecError::InvalidWorkingDir { path, .. } => {
                assert_eq!(path, "/tmp/foo");
            }
            _ => panic!("expected InvalidWorkingDir"),
        }
    }

    #[test]
    fn test_map_working_dir_workspacex_rejected() {
        // Paths like /workspaceXYZ should be rejected (no slash separator)
        let err = map_working_dir_impl("/workspacedata", None, Path::new(HOST_WS)).unwrap_err();
        match err {
            HostExecError::InvalidWorkingDir { path, reason } => {
                assert_eq!(path, "/workspacedata");
                assert_eq!(reason, "invalid working_dir");
            }
            _ => panic!("expected InvalidWorkingDir"),
        }
    }

    #[test]
    fn test_map_working_dir_different_host_workspace() {
        // Verify behavior with a non-default host_workspace path
        let custom_host_ws = "/opt/custom/workspace";
        let slot = std::path::PathBuf::from("/opt/custom/workspace/pool/myproj/1");
        let result =
            map_working_dir_impl("/workspace/src", Some(&slot), Path::new(custom_host_ws)).unwrap();
        assert_eq!(result, "%WORKSPACE%/pool/myproj/1/src");
    }

    #[test]
    fn test_map_working_dir_slot_completely_different_prefix() {
        // Slot path with a completely different prefix from host_workspace
        let slot = std::path::PathBuf::from("/opt/other/projects/myapp");
        let result = map_working_dir_impl("/workspace", Some(&slot), Path::new(HOST_WS)).unwrap();
        assert_eq!(result, "/opt/other/projects/myapp");
    }

    #[test]
    fn test_map_working_dir_slot_completely_different_prefix_subdir() {
        let slot = std::path::PathBuf::from("/opt/other/projects/myapp");
        let result =
            map_working_dir_impl("/workspace/src", Some(&slot), Path::new(HOST_WS)).unwrap();
        assert_eq!(result, "/opt/other/projects/myapp/src");
    }

    // --- validate_script_path tests ---

    #[test]
    fn validate_script_path_valid() {
        assert_eq!(
            validate_script_path("/workspace/scripts/deploy.sh").unwrap(),
            "scripts/deploy.sh"
        );
    }

    #[test]
    fn validate_script_path_nested() {
        assert_eq!(
            validate_script_path("/workspace/a/b/c.sh").unwrap(),
            "a/b/c.sh"
        );
    }

    #[test]
    fn validate_script_path_rejects_no_workspace_prefix() {
        assert!(validate_script_path("/tmp/script.sh").is_err());
        assert!(validate_script_path("scripts/deploy.sh").is_err());
    }

    #[test]
    fn validate_script_path_rejects_workspace_root_only() {
        // /workspace/ alone (empty rel_path after stripping)
        assert!(validate_script_path("/workspace/").is_err());
    }

    #[test]
    fn validate_script_path_rejects_dotdot() {
        assert!(validate_script_path("/workspace/../etc/passwd").is_err());
        assert!(validate_script_path("/workspace/scripts/../other.sh").is_err());
    }

    #[test]
    fn validate_script_path_rejects_double_slash() {
        // Double slash produces an empty segment
        assert!(validate_script_path("/workspace/scripts//deploy.sh").is_err());
    }

    // --- map_script_path_impl tests ---

    #[test]
    fn map_script_path_workspace_mount() {
        let result =
            map_script_path_impl("/workspace/scripts/run.sh", None, Path::new(HOST_WS)).unwrap();
        assert_eq!(result, "%WORKSPACE%/scripts/run.sh");
    }

    #[test]
    fn map_script_path_pool_mount() {
        let slot = std::path::PathBuf::from("/home/user/.ur/workspace/pool/proj/0");
        let result =
            map_script_path_impl("/workspace/scripts/run.sh", Some(&slot), Path::new(HOST_WS))
                .unwrap();
        assert_eq!(result, "%WORKSPACE%/pool/proj/0/scripts/run.sh");
    }

    #[test]
    fn map_script_path_workspace_slot() {
        let slot = std::path::PathBuf::from("/Users/foo/myproject");
        let result =
            map_script_path_impl("/workspace/scripts/run.sh", Some(&slot), Path::new(HOST_WS))
                .unwrap();
        assert_eq!(result, "/Users/foo/myproject/scripts/run.sh");
    }
}
