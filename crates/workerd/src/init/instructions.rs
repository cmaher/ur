use std::path::PathBuf;

use tracing::{info, warn};
use ur_config::AgentType;

use super::collect_md_files;

const POTENTIAL_INSTRUCTIONS_DIR: &str = ".agent-shared/instructions";
const SHARED_INSTRUCTIONS_DIR: &str = ".agent-shared/shared-instructions";

/// Manages composing the project instruction file (e.g. `~/.claude/CLAUDE.md`)
/// from a strategy file, shared fragments, and an optional project reference.
#[derive(Clone)]
pub struct InitInstructionsManager {
    home: PathBuf,
    agent: AgentType,
}

impl InitInstructionsManager {
    pub fn new(home: PathBuf, agent: AgentType) -> Self {
        InitInstructionsManager { home, agent }
    }

    fn instruction_dest(&self) -> PathBuf {
        self.home
            .join(self.agent.customization_root())
            .join(self.agent.instruction_filename())
    }

    fn project_instruction_dest(&self) -> PathBuf {
        self.home
            .join(self.agent.customization_root())
            .join(format!("PROJECT_{}", self.agent.instruction_filename()))
    }

    pub async fn run(&self) -> Result<(), std::io::Error> {
        let strategy_name = match std::env::var(ur_config::UR_WORKER_INSTRUCTION_STRATEGY_ENV) {
            Ok(val) if !val.trim().is_empty() => val,
            _ => {
                info!(
                    env = ur_config::UR_WORKER_INSTRUCTION_STRATEGY_ENV,
                    "env var empty or missing, skipping instruction file setup"
                );
                return Ok(());
            }
        };

        let strategy_src = self
            .home
            .join(POTENTIAL_INSTRUCTIONS_DIR)
            .join(format!("{strategy_name}.md"));
        let dst = self.instruction_dest();

        if !strategy_src.exists() {
            warn!(
                name = %strategy_name,
                path = %strategy_src.display(),
                "strategy instruction file not found in potential instructions"
            );
            return Ok(());
        }

        // Compose final instruction file: strategy file + all shared fragments
        let mut content = tokio::fs::read_to_string(&strategy_src).await?;
        info!(
            name = %strategy_name,
            src = %strategy_src.display(),
            "read strategy instruction file"
        );

        let shared_dir = self.home.join(SHARED_INSTRUCTIONS_DIR);
        let shared_files = collect_md_files(&shared_dir).await;
        for path in &shared_files {
            let shared_content = tokio::fs::read_to_string(path).await?;
            content.push_str("\n\n");
            content.push_str(&shared_content);
            info!(path = %path.display(), "appended shared instruction fragment");
        }

        // AGY does not read workspace CLAUDE.md and has no fallback-filename
        // setting, so fold project instructions into its global AGENTS.md.
        if let Some(project_content) = self.resolve_project_instruction().await? {
            if self.agent == AgentType::Agy {
                content.push_str("\n\n");
                content.push_str(&project_content);
            } else {
                let project_dest = self.project_instruction_dest();
                tokio::fs::write(&project_dest, &project_content).await?;
                info!(dst = %project_dest.display(), "wrote project instruction file");

                content.push_str("\n\n@");
                content.push_str(&project_dest.to_string_lossy());
            }
        }

        tokio::fs::write(&dst, &content).await?;
        info!(dst = %dst.display(), "wrote composed instruction file");

        Ok(())
    }

    /// Read the project instruction file (if `UR_PROJECT_INSTRUCTION` is set), resolve
    /// `%WORKSPACE%` placeholders using `UR_HOST_WORKSPACE`, and return the resolved content.
    async fn resolve_project_instruction(&self) -> Result<Option<String>, std::io::Error> {
        let project_path = match std::env::var(ur_config::UR_PROJECT_INSTRUCTION_ENV) {
            Ok(val) if !val.trim().is_empty() => val,
            _ => return Ok(None),
        };

        let raw_content = tokio::fs::read_to_string(&project_path).await?;
        info!(path = %project_path, "read project instruction file");

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
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::sync::Mutex;

    // Serialize tests that modify env vars
    static ENV_LOCK: Mutex<()> = Mutex::const_new(());

    fn setup_strategy_file(tmp: &TempDir, name: &str, content: &str) {
        let potential_dir = tmp.path().join(POTENTIAL_INSTRUCTIONS_DIR);
        std::fs::create_dir_all(&potential_dir).unwrap();
        std::fs::write(potential_dir.join(format!("{name}.md")), content).unwrap();
        // Ensure .claude dir exists for destination
        std::fs::create_dir_all(tmp.path().join(".claude")).unwrap();
    }

    #[tokio::test]
    async fn run_copies_strategy_file() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = TempDir::new().unwrap();
        setup_strategy_file(&tmp, "code", "# Code Worker\nBe a coder.");

        // SAFETY: tests are serialized via ENV_LOCK
        unsafe { std::env::set_var(ur_config::UR_WORKER_INSTRUCTION_STRATEGY_ENV, "code") };
        let mgr = InitInstructionsManager::new(tmp.path().to_path_buf(), AgentType::Claude);
        mgr.run().await.unwrap();
        unsafe { std::env::remove_var(ur_config::UR_WORKER_INSTRUCTION_STRATEGY_ENV) };

        let dest = tmp.path().join(".claude/CLAUDE.md");
        assert!(dest.exists(), "CLAUDE.md should be created");
        let content = std::fs::read_to_string(&dest).unwrap();
        assert_eq!(content, "# Code Worker\nBe a coder.");
    }

    #[tokio::test]
    async fn run_composes_shared_fragments() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = TempDir::new().unwrap();
        setup_strategy_file(&tmp, "code", "# Code Worker");

        // Create shared-instructions with two fragments
        let shared_dir = tmp.path().join(SHARED_INSTRUCTIONS_DIR);
        std::fs::create_dir_all(&shared_dir).unwrap();
        std::fs::write(shared_dir.join("alpha.md"), "# Alpha").unwrap();
        std::fs::write(shared_dir.join("beta.md"), "# Beta").unwrap();
        // Non-.md files should be ignored
        std::fs::write(shared_dir.join("ignore.txt"), "nope").unwrap();

        // SAFETY: tests are serialized via ENV_LOCK
        unsafe { std::env::set_var(ur_config::UR_WORKER_INSTRUCTION_STRATEGY_ENV, "code") };
        let mgr = InitInstructionsManager::new(tmp.path().to_path_buf(), AgentType::Claude);
        mgr.run().await.unwrap();
        unsafe { std::env::remove_var(ur_config::UR_WORKER_INSTRUCTION_STRATEGY_ENV) };

        let content = std::fs::read_to_string(tmp.path().join(".claude/CLAUDE.md")).unwrap();
        assert_eq!(content, "# Code Worker\n\n# Alpha\n\n# Beta");
    }

    #[tokio::test]
    async fn run_missing_file_warns_but_succeeds() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join(".claude")).unwrap();

        // SAFETY: tests are serialized via ENV_LOCK
        unsafe { std::env::set_var(ur_config::UR_WORKER_INSTRUCTION_STRATEGY_ENV, "nonexistent") };
        let mgr = InitInstructionsManager::new(tmp.path().to_path_buf(), AgentType::Claude);
        let result = mgr.run().await;
        unsafe { std::env::remove_var(ur_config::UR_WORKER_INSTRUCTION_STRATEGY_ENV) };

        assert!(result.is_ok(), "missing file should not cause an error");
        let dest = tmp.path().join(".claude/CLAUDE.md");
        assert!(!dest.exists(), "CLAUDE.md should not be created");
    }

    #[tokio::test]
    async fn run_unset_env_skips() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join(".claude")).unwrap();

        // SAFETY: tests are serialized via ENV_LOCK
        unsafe { std::env::remove_var(ur_config::UR_WORKER_INSTRUCTION_STRATEGY_ENV) };
        let mgr = InitInstructionsManager::new(tmp.path().to_path_buf(), AgentType::Claude);
        let result = mgr.run().await;

        assert!(result.is_ok(), "unset env should not cause an error");
        let dest = tmp.path().join(".claude/CLAUDE.md");
        assert!(!dest.exists(), "CLAUDE.md should not be created");
    }

    #[tokio::test]
    async fn run_with_project_instruction() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = TempDir::new().unwrap();
        setup_strategy_file(&tmp, "code", "# Code Worker");

        // Write a project instruction file
        let project_dir = tmp.path().join("project-instruction");
        std::fs::create_dir_all(&project_dir).unwrap();
        let project_path = project_dir.join("CLAUDE.md");
        std::fs::write(&project_path, "# Project\nWorkspace: %WORKSPACE%/src").unwrap();

        // SAFETY: tests are serialized via ENV_LOCK
        unsafe {
            std::env::set_var(ur_config::UR_WORKER_INSTRUCTION_STRATEGY_ENV, "code");
            std::env::set_var(
                ur_config::UR_PROJECT_INSTRUCTION_ENV,
                project_path.to_str().unwrap(),
            );
            std::env::set_var(ur_config::UR_HOST_WORKSPACE_ENV, "/host/workspace");
        };

        let mgr = InitInstructionsManager::new(tmp.path().to_path_buf(), AgentType::Claude);
        mgr.run().await.unwrap();

        unsafe {
            std::env::remove_var(ur_config::UR_WORKER_INSTRUCTION_STRATEGY_ENV);
            std::env::remove_var(ur_config::UR_PROJECT_INSTRUCTION_ENV);
            std::env::remove_var(ur_config::UR_HOST_WORKSPACE_ENV);
        };

        // PROJECT_CLAUDE.md should exist with resolved content
        let project_dest = tmp.path().join(".claude/PROJECT_CLAUDE.md");
        assert!(project_dest.exists(), "PROJECT_CLAUDE.md should be created");
        let project_content = std::fs::read_to_string(&project_dest).unwrap();
        assert_eq!(project_content, "# Project\nWorkspace: /host/workspace/src");

        // CLAUDE.md should contain @ reference
        let claude_content = std::fs::read_to_string(tmp.path().join(".claude/CLAUDE.md")).unwrap();
        let expected_ref = format!("\n\n@{}", project_dest.display());
        assert!(
            claude_content.ends_with(&expected_ref),
            "CLAUDE.md should end with @ reference, got: {claude_content}"
        );
    }

    #[tokio::test]
    async fn run_without_project_instruction() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = TempDir::new().unwrap();
        setup_strategy_file(&tmp, "code", "# Code Worker");

        // SAFETY: tests are serialized via ENV_LOCK
        unsafe {
            std::env::set_var(ur_config::UR_WORKER_INSTRUCTION_STRATEGY_ENV, "code");
            std::env::remove_var(ur_config::UR_PROJECT_INSTRUCTION_ENV);
            std::env::remove_var(ur_config::UR_HOST_WORKSPACE_ENV);
        };

        let mgr = InitInstructionsManager::new(tmp.path().to_path_buf(), AgentType::Claude);
        mgr.run().await.unwrap();

        unsafe { std::env::remove_var(ur_config::UR_WORKER_INSTRUCTION_STRATEGY_ENV) };

        // PROJECT_CLAUDE.md should not exist
        let project_dest = tmp.path().join(".claude/PROJECT_CLAUDE.md");
        assert!(
            !project_dest.exists(),
            "PROJECT_CLAUDE.md should not be created"
        );

        // CLAUDE.md should not contain @ reference
        let claude_content = std::fs::read_to_string(tmp.path().join(".claude/CLAUDE.md")).unwrap();
        assert_eq!(claude_content, "# Code Worker");
    }

    #[tokio::test]
    async fn run_project_instruction_without_host_workspace() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = TempDir::new().unwrap();
        setup_strategy_file(&tmp, "code", "# Code Worker");

        // Write a project instruction file with %WORKSPACE% but don't set UR_HOST_WORKSPACE
        let project_dir = tmp.path().join("project-instruction");
        std::fs::create_dir_all(&project_dir).unwrap();
        let project_path = project_dir.join("CLAUDE.md");
        std::fs::write(&project_path, "# Project\nPath: %WORKSPACE%/foo").unwrap();

        // SAFETY: tests are serialized via ENV_LOCK
        unsafe {
            std::env::set_var(ur_config::UR_WORKER_INSTRUCTION_STRATEGY_ENV, "code");
            std::env::set_var(
                ur_config::UR_PROJECT_INSTRUCTION_ENV,
                project_path.to_str().unwrap(),
            );
            std::env::remove_var(ur_config::UR_HOST_WORKSPACE_ENV);
        };

        let mgr = InitInstructionsManager::new(tmp.path().to_path_buf(), AgentType::Claude);
        mgr.run().await.unwrap();

        unsafe {
            std::env::remove_var(ur_config::UR_WORKER_INSTRUCTION_STRATEGY_ENV);
            std::env::remove_var(ur_config::UR_PROJECT_INSTRUCTION_ENV);
        };

        // PROJECT_CLAUDE.md should exist with unresolved %WORKSPACE%
        let project_dest = tmp.path().join(".claude/PROJECT_CLAUDE.md");
        assert!(project_dest.exists(), "PROJECT_CLAUDE.md should be created");
        let project_content = std::fs::read_to_string(&project_dest).unwrap();
        assert_eq!(
            project_content, "# Project\nPath: %WORKSPACE%/foo",
            "content should pass through unchanged without UR_HOST_WORKSPACE"
        );
    }

    #[tokio::test]
    async fn agy_uses_customization_root_and_folds_project_instructions() {
        let _lock = ENV_LOCK.lock().await;
        let tmp = TempDir::new().unwrap();
        setup_strategy_file(&tmp, "code", "# Code Worker");
        std::fs::create_dir_all(tmp.path().join(".gemini/config")).unwrap();
        let project_path = tmp.path().join("CLAUDE.md");
        std::fs::write(&project_path, "# Project rules").unwrap();

        unsafe {
            std::env::set_var(ur_config::UR_WORKER_INSTRUCTION_STRATEGY_ENV, "code");
            std::env::set_var(ur_config::UR_PROJECT_INSTRUCTION_ENV, &project_path);
        }
        InitInstructionsManager::new(tmp.path().to_path_buf(), AgentType::Agy)
            .run()
            .await
            .unwrap();
        unsafe {
            std::env::remove_var(ur_config::UR_WORKER_INSTRUCTION_STRATEGY_ENV);
            std::env::remove_var(ur_config::UR_PROJECT_INSTRUCTION_ENV);
        }

        assert_eq!(
            std::fs::read_to_string(tmp.path().join(".gemini/config/AGENTS.md")).unwrap(),
            "# Code Worker\n\n# Project rules"
        );
        assert!(!tmp.path().join(".gemini/config/PROJECT_AGENTS.md").exists());
    }
}
