use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, Result, bail};
use tokio::process::Command;
use tracing::info;

const IN_REPO_HOOKS_DIR: &str = "ur-hooks";
const SYNC_HOOKS_DIR: &str = "startup";
const BACKGROUND_HOOKS_DIR: &str = "startup-bg";

#[derive(Clone, Copy, Debug)]
pub enum HookKind {
    Sync,
    Background,
}

impl HookKind {
    fn directory(self) -> &'static str {
        match self {
            Self::Sync => SYNC_HOOKS_DIR,
            Self::Background => BACKGROUND_HOOKS_DIR,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SpawnedHook {
    pub name: String,
    pub pid: u32,
}

/// Resolves and runs project startup hooks from the repository and host overlay.
#[derive(Clone)]
pub struct StartupHooksManager {
    workspace: PathBuf,
    host_overlay: PathBuf,
}

impl StartupHooksManager {
    pub fn new(workspace: PathBuf, host_overlay: PathBuf) -> Self {
        Self {
            workspace,
            host_overlay,
        }
    }

    /// Resolve executable hooks by filename, with the host overlay winning conflicts.
    pub fn resolve(&self, kind: HookKind) -> Result<Vec<PathBuf>> {
        let directory = kind.directory();
        let mut hooks = BTreeMap::new();
        self.collect_layer(
            &self.workspace.join(IN_REPO_HOOKS_DIR).join(directory),
            &mut hooks,
        )?;
        self.collect_layer(&self.host_overlay.join(directory), &mut hooks)?;
        Ok(hooks.into_values().collect())
    }

    /// Run synchronous hooks in order, then detach each background hook.
    pub async fn run(&self) -> Result<Vec<SpawnedHook>> {
        for hook in self.resolve(HookKind::Sync)? {
            self.run_sync_hook(&hook).await?;
        }

        let mut spawned = Vec::new();
        for hook in self.resolve(HookKind::Background)? {
            spawned.push(self.spawn_background_hook(&hook)?);
        }
        Ok(spawned)
    }

    fn collect_layer(&self, directory: &Path, hooks: &mut BTreeMap<String, PathBuf>) -> Result<()> {
        let entries = match std::fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("reading startup hooks {}", directory.display()));
            }
        };

        for entry in entries {
            let entry = entry.with_context(|| {
                format!("reading startup hook entry in {}", directory.display())
            })?;
            let path = entry.path();
            if entry.file_type()?.is_file() && is_executable(&path)? {
                hooks.insert(entry.file_name().to_string_lossy().into_owned(), path);
            }
        }
        Ok(())
    }

    async fn run_sync_hook(&self, hook: &Path) -> Result<()> {
        let name = hook_name(hook);
        info!(hook = %name, path = %hook.display(), "running startup hook");
        let output = Command::new(hook)
            .current_dir(&self.workspace)
            .output()
            .await
            .with_context(|| format!("running startup hook {name}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            bail!(
                "startup hook {name} failed with status {}: {stderr}",
                output.status
            );
        }
        Ok(())
    }

    fn spawn_background_hook(&self, hook: &Path) -> Result<SpawnedHook> {
        let name = hook_name(hook);
        let child = Command::new(hook)
            .current_dir(&self.workspace)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("spawning background startup hook {name}"))?;
        let pid = child
            .id()
            .with_context(|| format!("background startup hook {name} has no process id"))?;
        Ok(SpawnedHook { name, pid })
    }
}

fn hook_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

#[cfg(unix)]
fn is_executable(path: &Path) -> Result<bool> {
    use std::os::unix::fs::PermissionsExt;
    Ok(std::fs::metadata(path)?.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(_path: &Path) -> Result<bool> {
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use tempfile::TempDir;

    fn write_hook(root: &Path, kind: &str, name: &str, body: &str, executable: bool) {
        let path = root.join(kind).join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, body).unwrap();
        let mode = if executable { 0o755 } else { 0o644 };
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    fn manager(tmp: &TempDir) -> StartupHooksManager {
        StartupHooksManager::new(tmp.path().join("workspace"), tmp.path().join("host"))
    }

    async fn wait_for_file(path: &Path) -> bool {
        for _ in 0..500 {
            if path.exists() {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        false
    }

    #[test]
    fn resolve_overlays_by_filename_in_lexical_order() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace/ur-hooks");
        let host = tmp.path().join("host");
        write_hook(&workspace, "startup", "20-shared", "repo", true);
        write_hook(&workspace, "startup", "30-repo", "repo", true);
        write_hook(&host, "startup", "10-host", "host", true);
        write_hook(&host, "startup", "20-shared", "host", true);

        let hooks = manager(&tmp).resolve(HookKind::Sync).unwrap();

        assert_eq!(
            hooks,
            vec![
                host.join("startup/10-host"),
                host.join("startup/20-shared"),
                workspace.join("startup/30-repo"),
            ]
        );
    }

    #[test]
    fn resolve_uses_background_directories() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace/ur-hooks");
        write_hook(&workspace, "startup-bg", "10-bg", "background", true);

        assert_eq!(
            manager(&tmp).resolve(HookKind::Background).unwrap(),
            vec![workspace.join("startup-bg/10-bg")]
        );
    }

    #[test]
    fn resolve_ignores_missing_directories_and_non_executable_files() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace/ur-hooks");
        write_hook(&workspace, "startup", "10-not-executable", "ignored", false);

        assert!(manager(&tmp).resolve(HookKind::Sync).unwrap().is_empty());
    }

    #[tokio::test]
    async fn run_stops_at_first_failing_sync_hook() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace/ur-hooks");
        let marker = tmp.path().join("should-not-exist");
        write_hook(
            &workspace,
            "startup",
            "10-fail",
            "#!/bin/sh\necho hook-stderr >&2\nexit 7\n",
            true,
        );
        write_hook(
            &workspace,
            "startup-bg",
            "20-background",
            &format!("#!/bin/sh\ntouch {}\n", marker.display()),
            true,
        );

        let error = manager(&tmp).run().await.unwrap_err().to_string();

        assert!(error.contains("10-fail"), "{error}");
        assert!(error.contains("7"), "{error}");
        assert!(error.contains("hook-stderr"), "{error}");
        assert!(!marker.exists());
    }

    #[tokio::test]
    async fn run_returns_background_hook_names_and_pids() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace/ur-hooks");
        let marker = tmp.path().join("background-ran");
        write_hook(
            &workspace,
            "startup-bg",
            "10-background",
            &format!("#!/bin/sh\ntouch {}\n", marker.display()),
            true,
        );

        let spawned = manager(&tmp).run().await.unwrap();

        assert_eq!(spawned.len(), 1);
        assert_eq!(spawned[0].name, "10-background");
        assert!(spawned[0].pid > 0);
        assert!(wait_for_file(&marker).await, "background hook did not run");
    }
}
