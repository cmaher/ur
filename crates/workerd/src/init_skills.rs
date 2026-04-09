use std::path::PathBuf;

use tracing::{info, warn};

const SKILLS_ENV: &str = "UR_WORKER_SKILLS";
const POTENTIAL_SKILLS_DIR: &str = ".claude/potential-skills";
const SKILLS_DIR: &str = ".claude/skills";
const CLAUDE_ENV: &str = "UR_WORKER_CLAUDE";
const CAVEMAN_ENV: &str = "UR_WORKER_CAVEMAN";
const POTENTIAL_CLAUDES_DIR: &str = ".claude/potential-claudes";
const SHARED_CLAUDES_DIR: &str = ".claude/shared-claudes";
const CLAUDE_MD_DEST: &str = ".claude/CLAUDE.md";
const PROJECT_CLAUDE_MD_DEST: &str = ".claude/PROJECT_CLAUDE.md";

/// Manages skill directory initialization from potential-skills based on an env var.
#[derive(Clone)]
pub struct InitSkillsManager {
    home: PathBuf,
}

impl InitSkillsManager {
    pub fn from_env() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| ur_config::WORKER_HOME.into());
        InitSkillsManager {
            home: PathBuf::from(home),
        }
    }

    pub async fn run(&self) -> i32 {
        if let Err(e) = self.init_skills().await {
            eprintln!("init-skills failed: {e}");
            return 1;
        }
        if let Err(e) = self.init_claude_md().await {
            eprintln!("init-claude-md failed: {e}");
            return 1;
        }
        0
    }

    async fn init_skills(&self) -> Result<(), std::io::Error> {
        let skills_dir = self.home.join(SKILLS_DIR);
        let potential_dir = self.home.join(POTENTIAL_SKILLS_DIR);

        // Always wipe and recreate skills dir
        if skills_dir.exists() {
            tokio::fs::remove_dir_all(&skills_dir).await?;
            info!(path = %skills_dir.display(), "removed existing skills directory");
        }
        tokio::fs::create_dir_all(&skills_dir).await?;
        info!(path = %skills_dir.display(), "created skills directory");

        let skill_names = match std::env::var(SKILLS_ENV) {
            Ok(val) if !val.trim().is_empty() => val,
            _ => {
                info!(
                    env = SKILLS_ENV,
                    "env var empty or missing, no skills to copy"
                );
                return Ok(());
            }
        };

        let names: Vec<&str> = skill_names.split(',').map(|s| s.trim()).collect();
        info!(count = names.len(), skills = %skill_names, "processing skill list");

        for name in names {
            if name.is_empty() {
                continue;
            }
            let src = potential_dir.join(name);
            let dst = skills_dir.join(name);

            if !src.exists() {
                warn!(skill = name, path = %src.display(), "skill not found in potential-skills");
                continue;
            }

            copy_dir_recursive(&src, &dst).await?;
            info!(skill = name, src = %src.display(), dst = %dst.display(), "copied skill");
        }

        Ok(())
    }

    async fn init_claude_md(&self) -> Result<(), std::io::Error> {
        let claude_name = match std::env::var(CLAUDE_ENV) {
            Ok(val) if !val.trim().is_empty() => val,
            _ => {
                info!(
                    env = CLAUDE_ENV,
                    "env var empty or missing, skipping CLAUDE.md setup"
                );
                return Ok(());
            }
        };

        let strategy_src = self
            .home
            .join(POTENTIAL_CLAUDES_DIR)
            .join(format!("{claude_name}.md"));
        let dst = self.home.join(CLAUDE_MD_DEST);

        if !strategy_src.exists() {
            warn!(
                name = %claude_name,
                path = %strategy_src.display(),
                "strategy CLAUDE.md not found in potential-claudes"
            );
            return Ok(());
        }

        // Compose final CLAUDE.md: strategy file + all shared files
        let mut content = tokio::fs::read_to_string(&strategy_src).await?;
        info!(
            name = %claude_name,
            src = %strategy_src.display(),
            "read strategy CLAUDE.md"
        );

        let shared_dir = self.home.join(SHARED_CLAUDES_DIR);
        let shared_files = collect_md_files(&shared_dir).await;
        for path in &shared_files {
            let shared_content = tokio::fs::read_to_string(path).await?;
            content.push_str("\n\n");
            content.push_str(&shared_content);
            info!(path = %path.display(), "appended shared CLAUDE.md fragment");
        }

        // Append caveman fragment if UR_WORKER_CAVEMAN is set
        if let Ok(level) = std::env::var(CAVEMAN_ENV)
            && !level.trim().is_empty()
        {
            let caveman_src = self
                .home
                .join(POTENTIAL_CLAUDES_DIR)
                .join(format!("caveman-{level}.md"));
            match tokio::fs::read_to_string(&caveman_src).await {
                Ok(caveman_content) => {
                    content.push_str("\n\n");
                    content.push_str(&caveman_content);
                    info!(
                        level = %level,
                        path = %caveman_src.display(),
                        "appended caveman fragment"
                    );
                }
                Err(_) => {
                    warn!(
                        level = %level,
                        path = %caveman_src.display(),
                        "caveman fragment not found in potential-claudes"
                    );
                }
            }
        }

        // If a project CLAUDE.md is provided, resolve %WORKSPACE% and append @ reference
        if let Some(project_content) = self.resolve_project_claude().await? {
            let project_dest = self.home.join(PROJECT_CLAUDE_MD_DEST);
            tokio::fs::write(&project_dest, &project_content).await?;
            info!(dst = %project_dest.display(), "wrote PROJECT_CLAUDE.md");

            content.push_str("\n\n@");
            content.push_str(&self.home.join(PROJECT_CLAUDE_MD_DEST).to_string_lossy());
        }

        tokio::fs::write(&dst, &content).await?;
        info!(dst = %dst.display(), "wrote composed CLAUDE.md");

        Ok(())
    }

    /// Read the project CLAUDE.md (if UR_PROJECT_CLAUDE is set), resolve %WORKSPACE%
    /// placeholders using UR_HOST_WORKSPACE, and return the resolved content.
    async fn resolve_project_claude(&self) -> Result<Option<String>, std::io::Error> {
        let project_path = match std::env::var(ur_config::UR_PROJECT_CLAUDE_ENV) {
            Ok(val) if !val.trim().is_empty() => val,
            _ => return Ok(None),
        };

        let raw_content = tokio::fs::read_to_string(&project_path).await?;
        info!(path = %project_path, "read project CLAUDE.md");

        let resolved = match std::env::var(ur_config::UR_HOST_WORKSPACE_ENV) {
            Ok(workspace) if !workspace.trim().is_empty() => {
                ur_config::resolve_workspace_content(&raw_content, &workspace)
            }
            _ => raw_content,
        };

        Ok(Some(resolved))
    }
}

#[cfg(test)]
impl InitSkillsManager {
    fn with_home(home: PathBuf) -> Self {
        InitSkillsManager { home }
    }
}

async fn copy_dir_recursive(src: &PathBuf, dst: &PathBuf) -> Result<(), std::io::Error> {
    tokio::fs::create_dir_all(dst).await?;

    let mut entries = tokio::fs::read_dir(src).await?;
    while let Some(entry) = entries.next_entry().await? {
        let entry_type = entry.file_type().await?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if entry_type.is_dir() {
            Box::pin(copy_dir_recursive(
                &src_path.to_path_buf(),
                &dst_path.to_path_buf(),
            ))
            .await?;
        } else {
            tokio::fs::copy(&src_path, &dst_path).await?;
        }
    }

    Ok(())
}

/// Collect sorted `.md` file paths from a directory. Returns empty vec if dir doesn't exist.
async fn collect_md_files(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return files;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "md") {
            files.push(path);
        }
    }
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::sync::Mutex;

    // Serialize tests that modify env vars
    static ENV_LOCK: Mutex<()> = Mutex::const_new(());

    fn setup_claude_dir(tmp: &TempDir, name: &str, content: &str) {
        let potential_dir = tmp.path().join(POTENTIAL_CLAUDES_DIR);
        std::fs::create_dir_all(&potential_dir).unwrap();
        std::fs::write(potential_dir.join(format!("{name}.md")), content).unwrap();
        // Ensure .claude dir exists for destination
        std::fs::create_dir_all(tmp.path().join(".claude")).unwrap();
    }

    #[tokio::test]
    async fn init_claude_md_copies_strategy_file() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = TempDir::new().unwrap();
        setup_claude_dir(&tmp, "code", "# Code Worker\nBe a coder.");

        // SAFETY: tests are serialized via ENV_LOCK
        unsafe { std::env::set_var(CLAUDE_ENV, "code") };
        let mgr = InitSkillsManager::with_home(tmp.path().to_path_buf());
        mgr.init_claude_md().await.unwrap();
        unsafe { std::env::remove_var(CLAUDE_ENV) };

        let dest = tmp.path().join(CLAUDE_MD_DEST);
        assert!(dest.exists(), "CLAUDE.md should be created");
        let content = std::fs::read_to_string(&dest).unwrap();
        assert_eq!(content, "# Code Worker\nBe a coder.");
    }

    #[tokio::test]
    async fn init_claude_md_composes_shared_fragments() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = TempDir::new().unwrap();
        setup_claude_dir(&tmp, "code", "# Code Worker");

        // Create shared-claudes with two fragments
        let shared_dir = tmp.path().join(SHARED_CLAUDES_DIR);
        std::fs::create_dir_all(&shared_dir).unwrap();
        std::fs::write(shared_dir.join("alpha.md"), "# Alpha").unwrap();
        std::fs::write(shared_dir.join("beta.md"), "# Beta").unwrap();
        // Non-.md files should be ignored
        std::fs::write(shared_dir.join("ignore.txt"), "nope").unwrap();

        // SAFETY: tests are serialized via ENV_LOCK
        unsafe { std::env::set_var(CLAUDE_ENV, "code") };
        let mgr = InitSkillsManager::with_home(tmp.path().to_path_buf());
        mgr.init_claude_md().await.unwrap();
        unsafe { std::env::remove_var(CLAUDE_ENV) };

        let content = std::fs::read_to_string(tmp.path().join(CLAUDE_MD_DEST)).unwrap();
        assert_eq!(content, "# Code Worker\n\n# Alpha\n\n# Beta");
    }

    #[tokio::test]
    async fn init_claude_md_missing_file_warns_but_succeeds() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join(".claude")).unwrap();

        // SAFETY: tests are serialized via ENV_LOCK
        unsafe { std::env::set_var(CLAUDE_ENV, "nonexistent") };
        let mgr = InitSkillsManager::with_home(tmp.path().to_path_buf());
        let result = mgr.init_claude_md().await;
        unsafe { std::env::remove_var(CLAUDE_ENV) };

        assert!(result.is_ok(), "missing file should not cause an error");
        let dest = tmp.path().join(CLAUDE_MD_DEST);
        assert!(!dest.exists(), "CLAUDE.md should not be created");
    }

    #[tokio::test]
    async fn init_claude_md_unset_env_skips() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join(".claude")).unwrap();

        // SAFETY: tests are serialized via ENV_LOCK
        unsafe { std::env::remove_var(CLAUDE_ENV) };
        let mgr = InitSkillsManager::with_home(tmp.path().to_path_buf());
        let result = mgr.init_claude_md().await;

        assert!(result.is_ok(), "unset env should not cause an error");
        let dest = tmp.path().join(CLAUDE_MD_DEST);
        assert!(!dest.exists(), "CLAUDE.md should not be created");
    }

    #[tokio::test]
    async fn init_claude_md_with_project_claude() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = TempDir::new().unwrap();
        setup_claude_dir(&tmp, "code", "# Code Worker");

        // Write a project CLAUDE.md file
        let project_dir = tmp.path().join("project-claude");
        std::fs::create_dir_all(&project_dir).unwrap();
        let project_path = project_dir.join("CLAUDE.md");
        std::fs::write(&project_path, "# Project\nWorkspace: %WORKSPACE%/src").unwrap();

        // SAFETY: tests are serialized via ENV_LOCK
        unsafe {
            std::env::set_var(CLAUDE_ENV, "code");
            std::env::set_var(
                ur_config::UR_PROJECT_CLAUDE_ENV,
                project_path.to_str().unwrap(),
            );
            std::env::set_var(ur_config::UR_HOST_WORKSPACE_ENV, "/host/workspace");
        };

        let mgr = InitSkillsManager::with_home(tmp.path().to_path_buf());
        mgr.init_claude_md().await.unwrap();

        unsafe {
            std::env::remove_var(CLAUDE_ENV);
            std::env::remove_var(ur_config::UR_PROJECT_CLAUDE_ENV);
            std::env::remove_var(ur_config::UR_HOST_WORKSPACE_ENV);
        };

        // PROJECT_CLAUDE.md should exist with resolved content
        let project_dest = tmp.path().join(PROJECT_CLAUDE_MD_DEST);
        assert!(project_dest.exists(), "PROJECT_CLAUDE.md should be created");
        let project_content = std::fs::read_to_string(&project_dest).unwrap();
        assert_eq!(project_content, "# Project\nWorkspace: /host/workspace/src");

        // CLAUDE.md should contain @ reference
        let claude_content = std::fs::read_to_string(tmp.path().join(CLAUDE_MD_DEST)).unwrap();
        let expected_ref = format!("\n\n@{}", project_dest.display());
        assert!(
            claude_content.ends_with(&expected_ref),
            "CLAUDE.md should end with @ reference, got: {claude_content}"
        );
    }

    #[tokio::test]
    async fn init_claude_md_without_project_claude() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = TempDir::new().unwrap();
        setup_claude_dir(&tmp, "code", "# Code Worker");

        // SAFETY: tests are serialized via ENV_LOCK
        unsafe {
            std::env::set_var(CLAUDE_ENV, "code");
            std::env::remove_var(ur_config::UR_PROJECT_CLAUDE_ENV);
            std::env::remove_var(ur_config::UR_HOST_WORKSPACE_ENV);
        };

        let mgr = InitSkillsManager::with_home(tmp.path().to_path_buf());
        mgr.init_claude_md().await.unwrap();

        unsafe { std::env::remove_var(CLAUDE_ENV) };

        // PROJECT_CLAUDE.md should not exist
        let project_dest = tmp.path().join(PROJECT_CLAUDE_MD_DEST);
        assert!(
            !project_dest.exists(),
            "PROJECT_CLAUDE.md should not be created"
        );

        // CLAUDE.md should not contain @ reference
        let claude_content = std::fs::read_to_string(tmp.path().join(CLAUDE_MD_DEST)).unwrap();
        assert_eq!(claude_content, "# Code Worker");
    }

    #[tokio::test]
    async fn init_claude_md_caveman_appended() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = TempDir::new().unwrap();
        setup_claude_dir(&tmp, "code", "# Code Worker");

        // Create caveman fragment
        let potential_dir = tmp.path().join(POTENTIAL_CLAUDES_DIR);
        std::fs::write(potential_dir.join("caveman-ultra.md"), "# Caveman Ultra").unwrap();

        unsafe {
            std::env::set_var(CLAUDE_ENV, "code");
            std::env::set_var(CAVEMAN_ENV, "ultra");
            std::env::remove_var(ur_config::UR_PROJECT_CLAUDE_ENV);
        };

        let mgr = InitSkillsManager::with_home(tmp.path().to_path_buf());
        mgr.init_claude_md().await.unwrap();

        unsafe {
            std::env::remove_var(CLAUDE_ENV);
            std::env::remove_var(CAVEMAN_ENV);
        };

        let content = std::fs::read_to_string(tmp.path().join(CLAUDE_MD_DEST)).unwrap();
        assert_eq!(content, "# Code Worker\n\n# Caveman Ultra");
    }

    #[tokio::test]
    async fn init_claude_md_caveman_skipped_when_unset() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = TempDir::new().unwrap();
        setup_claude_dir(&tmp, "code", "# Code Worker");

        unsafe {
            std::env::set_var(CLAUDE_ENV, "code");
            std::env::remove_var(CAVEMAN_ENV);
            std::env::remove_var(ur_config::UR_PROJECT_CLAUDE_ENV);
        };

        let mgr = InitSkillsManager::with_home(tmp.path().to_path_buf());
        mgr.init_claude_md().await.unwrap();

        unsafe { std::env::remove_var(CLAUDE_ENV) };

        let content = std::fs::read_to_string(tmp.path().join(CLAUDE_MD_DEST)).unwrap();
        assert_eq!(content, "# Code Worker");
    }

    #[tokio::test]
    async fn init_claude_md_caveman_missing_file_warns_no_error() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = TempDir::new().unwrap();
        setup_claude_dir(&tmp, "code", "# Code Worker");

        unsafe {
            std::env::set_var(CLAUDE_ENV, "code");
            std::env::set_var(CAVEMAN_ENV, "bogus");
            std::env::remove_var(ur_config::UR_PROJECT_CLAUDE_ENV);
        };

        let mgr = InitSkillsManager::with_home(tmp.path().to_path_buf());
        let result = mgr.init_claude_md().await;

        unsafe {
            std::env::remove_var(CLAUDE_ENV);
            std::env::remove_var(CAVEMAN_ENV);
        };

        assert!(
            result.is_ok(),
            "missing caveman file should not cause an error"
        );
        let content = std::fs::read_to_string(tmp.path().join(CLAUDE_MD_DEST)).unwrap();
        // No caveman content appended
        assert_eq!(content, "# Code Worker");
    }

    #[tokio::test]
    async fn init_claude_md_caveman_before_project_ref() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = TempDir::new().unwrap();
        setup_claude_dir(&tmp, "code", "# Code Worker");

        // Create caveman fragment
        let potential_dir = tmp.path().join(POTENTIAL_CLAUDES_DIR);
        std::fs::write(potential_dir.join("caveman-lite.md"), "# Caveman Lite").unwrap();

        // Create project CLAUDE.md
        let project_dir = tmp.path().join("project-claude");
        std::fs::create_dir_all(&project_dir).unwrap();
        let project_path = project_dir.join("CLAUDE.md");
        std::fs::write(&project_path, "# Project").unwrap();

        unsafe {
            std::env::set_var(CLAUDE_ENV, "code");
            std::env::set_var(CAVEMAN_ENV, "lite");
            std::env::set_var(
                ur_config::UR_PROJECT_CLAUDE_ENV,
                project_path.to_str().unwrap(),
            );
            std::env::remove_var(ur_config::UR_HOST_WORKSPACE_ENV);
        };

        let mgr = InitSkillsManager::with_home(tmp.path().to_path_buf());
        mgr.init_claude_md().await.unwrap();

        unsafe {
            std::env::remove_var(CLAUDE_ENV);
            std::env::remove_var(CAVEMAN_ENV);
            std::env::remove_var(ur_config::UR_PROJECT_CLAUDE_ENV);
        };

        let content = std::fs::read_to_string(tmp.path().join(CLAUDE_MD_DEST)).unwrap();
        let project_dest = tmp.path().join(PROJECT_CLAUDE_MD_DEST);
        let expected_ref = format!("\n\n@{}", project_dest.display());

        // Caveman should appear before the project @ reference
        let caveman_pos = content
            .find("# Caveman Lite")
            .expect("caveman content should be present");
        let ref_pos = content
            .find(&expected_ref)
            .expect("project ref should be present");
        assert!(
            caveman_pos < ref_pos,
            "caveman content should appear before project @ reference"
        );
    }

    #[tokio::test]
    async fn init_claude_md_project_claude_without_host_workspace() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = TempDir::new().unwrap();
        setup_claude_dir(&tmp, "code", "# Code Worker");

        // Write a project CLAUDE.md with %WORKSPACE% but don't set UR_HOST_WORKSPACE
        let project_dir = tmp.path().join("project-claude");
        std::fs::create_dir_all(&project_dir).unwrap();
        let project_path = project_dir.join("CLAUDE.md");
        std::fs::write(&project_path, "# Project\nPath: %WORKSPACE%/foo").unwrap();

        // SAFETY: tests are serialized via ENV_LOCK
        unsafe {
            std::env::set_var(CLAUDE_ENV, "code");
            std::env::set_var(
                ur_config::UR_PROJECT_CLAUDE_ENV,
                project_path.to_str().unwrap(),
            );
            std::env::remove_var(ur_config::UR_HOST_WORKSPACE_ENV);
        };

        let mgr = InitSkillsManager::with_home(tmp.path().to_path_buf());
        mgr.init_claude_md().await.unwrap();

        unsafe {
            std::env::remove_var(CLAUDE_ENV);
            std::env::remove_var(ur_config::UR_PROJECT_CLAUDE_ENV);
        };

        // PROJECT_CLAUDE.md should exist with unresolved %WORKSPACE%
        let project_dest = tmp.path().join(PROJECT_CLAUDE_MD_DEST);
        assert!(project_dest.exists(), "PROJECT_CLAUDE.md should be created");
        let project_content = std::fs::read_to_string(&project_dest).unwrap();
        assert_eq!(
            project_content, "# Project\nPath: %WORKSPACE%/foo",
            "content should pass through unchanged without UR_HOST_WORKSPACE"
        );
    }
}
