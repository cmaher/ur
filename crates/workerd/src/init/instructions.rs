use std::path::PathBuf;

use tracing::{info, warn};

use super::collect_md_files;
use super::inputs::{InstructionInputs, ProjectInstruction, ProjectInstructionMode};
use crate::paths::WorkerPaths;
use crate::skill_layout::SkillLayout;

const POTENTIAL_INSTRUCTIONS_DIR: &str = "instructions";
const SHARED_INSTRUCTIONS_DIR: &str = "shared-instructions";

/// Manages composing the project instruction file (e.g. `~/.claude/CLAUDE.md`)
/// from a strategy file, shared fragments, and an optional project reference.
#[derive(Clone)]
pub struct InitInstructionsManager {
    paths: WorkerPaths,
    layout: SkillLayout,
}

impl InitInstructionsManager {
    pub fn new(paths: WorkerPaths, layout: SkillLayout) -> Self {
        InitInstructionsManager { paths, layout }
    }

    /// `PROJECT_<instruction filename>` next to `instructions_dest`.
    fn project_instruction_dest(&self) -> Result<PathBuf, std::io::Error> {
        let dest = &self.paths.instructions_dest;
        let filename = dest.file_name().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("instructions_dest has no file name: {}", dest.display()),
            )
        })?;
        Ok(dest.with_file_name(format!("PROJECT_{}", filename.to_string_lossy())))
    }

    pub async fn run(&self, inputs: &InstructionInputs) -> Result<(), std::io::Error> {
        let Some(strategy_name) = inputs.strategy.as_deref() else {
            info!("no instruction strategy, skipping instruction file setup");
            return Ok(());
        };

        let strategy_src = self
            .paths
            .agent_shared
            .join(POTENTIAL_INSTRUCTIONS_DIR)
            .join(format!("{strategy_name}.md"));
        let dst = &self.paths.instructions_dest;

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

        let shared_dir = self.paths.agent_shared.join(SHARED_INSTRUCTIONS_DIR);
        let shared_files = collect_md_files(&shared_dir).await;
        for path in &shared_files {
            let shared_content = tokio::fs::read_to_string(path).await?;
            content.push_str("\n\n");
            content.push_str(&shared_content);
            info!(path = %path.display(), "appended shared instruction fragment");
        }

        // Namespaced-plugin skills coexist with the user's own skills; tell the
        // model to prefer the ur copy when both exist under the same name.
        if let Some(namespace) = self.layout.namespace() {
            content.push_str("\n\n");
            content.push_str(&skill_precedence_fragment(namespace));
            info!(namespace, "appended skill precedence fragment");
        }

        if let Some(forge) = inputs.forge.as_deref()
            && let Some(instructions) = forge_instructions(forge)
        {
            content.push_str("\n\n");
            content.push_str(instructions);
            info!(forge = %forge, "appended forge collaboration instructions");
        }

        if let Some(project) = &inputs.project {
            self.append_project_instruction(&mut content, project)
                .await?;
        }

        tokio::fs::write(dst, &content).await?;
        info!(dst = %dst.display(), "wrote composed instruction file");

        Ok(())
    }

    /// Read `project`, resolve `%WORKSPACE%` against its host workspace, and
    /// include it in `content` per its [`ProjectInstructionMode`].
    async fn append_project_instruction(
        &self,
        content: &mut String,
        project: &ProjectInstruction,
    ) -> Result<(), std::io::Error> {
        let raw_content = tokio::fs::read_to_string(&project.path).await?;
        info!(path = %project.path.display(), "read project instruction file");

        let resolved = match project.host_workspace.as_deref() {
            Some(workspace) => ur_config::resolve_workspace_content(&raw_content, workspace),
            None => raw_content,
        };

        content.push_str("\n\n");
        match project.mode {
            ProjectInstructionMode::Inline => content.push_str(&resolved),
            ProjectInstructionMode::Import => {
                let project_dest = self.project_instruction_dest()?;
                tokio::fs::write(&project_dest, &resolved).await?;
                info!(dst = %project_dest.display(), "wrote project instruction file");

                content.push('@');
                content.push_str(&project_dest.to_string_lossy());
            }
        }
        Ok(())
    }
}

/// Skill-precedence rule for a namespaced skill plugin (e.g. `ur`).
fn skill_precedence_fragment(namespace: &str) -> String {
    format!(
        "# Skill Precedence\n\n\
         Worker skills are installed as the `{namespace}` plugin and are invoked as \
         `{namespace}:<name>` (e.g. `/{namespace}:implement`). When a skill exists as both \
         `{namespace}:<name>` and `<name>`, ALWAYS use `{namespace}:<name>`.\n"
    )
}

fn forge_instructions(forge: &str) -> Option<&'static str> {
    match forge.trim().to_lowercase().as_str() {
        "gitea" => Some(
            "# Forge Collaboration\n\n\
             This repository is hosted on Gitea. \
             Use `tea` for pull requests, reviews, comments, and CI status checks (do not use `gh`).\n",
        ),
        "github" => Some(
            "# Forge Collaboration\n\n\
             This repository is hosted on GitHub. \
             Use `gh` for pull requests, reviews, comments, and CI status checks (do not use `tea`).\n",
        ),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::temp_paths;
    use tempfile::TempDir;
    use ur_config::AgentType;

    fn claude(tmp: &TempDir) -> WorkerPaths {
        temp_paths(tmp, AgentType::Claude)
    }

    fn strategy(name: &str) -> InstructionInputs {
        InstructionInputs {
            strategy: Some(name.into()),
            ..Default::default()
        }
    }

    fn with_forge(name: &str, forge: &str) -> InstructionInputs {
        InstructionInputs {
            forge: Some(forge.into()),
            ..strategy(name)
        }
    }

    fn with_project(
        name: &str,
        path: &std::path::Path,
        host_workspace: Option<&str>,
        mode: ProjectInstructionMode,
    ) -> InstructionInputs {
        InstructionInputs {
            project: Some(ProjectInstruction {
                path: path.to_path_buf(),
                host_workspace: host_workspace.map(str::to_owned),
                mode,
            }),
            ..strategy(name)
        }
    }

    async fn run(paths: WorkerPaths, layout: SkillLayout, inputs: &InstructionInputs) {
        InitInstructionsManager::new(paths, layout)
            .run(inputs)
            .await
            .unwrap();
    }

    fn setup_strategy_file(tmp: &TempDir, name: &str, content: &str) {
        let paths = claude(tmp);
        let potential_dir = paths.agent_shared.join(POTENTIAL_INSTRUCTIONS_DIR);
        std::fs::create_dir_all(&potential_dir).unwrap();
        std::fs::write(potential_dir.join(format!("{name}.md")), content).unwrap();
        std::fs::create_dir_all(paths.instructions_dest.parent().unwrap()).unwrap();
    }

    fn read_claude_md(tmp: &TempDir) -> String {
        std::fs::read_to_string(tmp.path().join("home/.claude/CLAUDE.md")).unwrap()
    }

    #[tokio::test]
    async fn run_copies_strategy_file() {
        let tmp = TempDir::new().unwrap();
        setup_strategy_file(&tmp, "code", "# Code Worker\nBe a coder.");

        run(claude(&tmp), SkillLayout::AgentHome, &strategy("code")).await;

        assert_eq!(read_claude_md(&tmp), "# Code Worker\nBe a coder.");
    }

    #[tokio::test]
    async fn run_composes_shared_fragments() {
        let tmp = TempDir::new().unwrap();
        setup_strategy_file(&tmp, "code", "# Code Worker");
        let shared_dir = claude(&tmp).agent_shared.join(SHARED_INSTRUCTIONS_DIR);
        std::fs::create_dir_all(&shared_dir).unwrap();
        std::fs::write(shared_dir.join("alpha.md"), "# Alpha").unwrap();
        std::fs::write(shared_dir.join("beta.md"), "# Beta").unwrap();
        // Non-.md files should be ignored
        std::fs::write(shared_dir.join("ignore.txt"), "nope").unwrap();

        run(claude(&tmp), SkillLayout::AgentHome, &strategy("code")).await;

        assert_eq!(read_claude_md(&tmp), "# Code Worker\n\n# Alpha\n\n# Beta");
    }

    #[tokio::test]
    async fn plugin_layout_appends_skill_precedence_after_shared_fragments() {
        let tmp = TempDir::new().unwrap();
        setup_strategy_file(&tmp, "code", "# Code Worker");
        let shared_dir = claude(&tmp).agent_shared.join(SHARED_INSTRUCTIONS_DIR);
        std::fs::create_dir_all(&shared_dir).unwrap();
        std::fs::write(shared_dir.join("alpha.md"), "# Alpha").unwrap();

        run(claude(&tmp), SkillLayout::ur_plugin(), &strategy("code")).await;

        let content = read_claude_md(&tmp);
        assert_eq!(
            content,
            format!(
                "# Code Worker\n\n# Alpha\n\n{}",
                skill_precedence_fragment("ur")
            )
        );
        assert!(content.contains(
            "When a skill exists as both `ur:<name>` and `<name>`, ALWAYS use `ur:<name>`."
        ));
    }

    #[tokio::test]
    async fn agent_home_layout_has_no_skill_precedence() {
        let tmp = TempDir::new().unwrap();
        setup_strategy_file(&tmp, "code", "# Code Worker");

        run(claude(&tmp), SkillLayout::AgentHome, &strategy("code")).await;

        assert!(!read_claude_md(&tmp).contains("# Skill Precedence"));
    }

    #[tokio::test]
    async fn run_missing_file_warns_but_succeeds() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("home/.claude")).unwrap();

        let result = InitInstructionsManager::new(claude(&tmp), SkillLayout::AgentHome)
            .run(&strategy("nonexistent"))
            .await;

        assert!(result.is_ok(), "missing file should not cause an error");
        assert!(!tmp.path().join("home/.claude/CLAUDE.md").exists());
    }

    #[tokio::test]
    async fn run_without_strategy_skips() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("home/.claude")).unwrap();

        let result = InitInstructionsManager::new(claude(&tmp), SkillLayout::AgentHome)
            .run(&InstructionInputs::default())
            .await;

        assert!(result.is_ok(), "no strategy should not cause an error");
        assert!(!tmp.path().join("home/.claude/CLAUDE.md").exists());
    }

    #[tokio::test]
    async fn run_with_project_instruction() {
        let tmp = TempDir::new().unwrap();
        setup_strategy_file(&tmp, "code", "# Code Worker");
        let project_dir = tmp.path().join("project-instruction");
        std::fs::create_dir_all(&project_dir).unwrap();
        let project_path = project_dir.join("CLAUDE.md");
        std::fs::write(&project_path, "# Project\nWorkspace: %WORKSPACE%/src").unwrap();

        let inputs = with_project(
            "code",
            &project_path,
            Some("/host/workspace"),
            ProjectInstructionMode::Import,
        );
        run(claude(&tmp), SkillLayout::AgentHome, &inputs).await;

        let project_dest = tmp.path().join("home/.claude/PROJECT_CLAUDE.md");
        assert_eq!(
            std::fs::read_to_string(&project_dest).unwrap(),
            "# Project\nWorkspace: /host/workspace/src"
        );
        let claude_content = read_claude_md(&tmp);
        let expected_ref = format!("\n\n@{}", project_dest.display());
        assert!(
            claude_content.ends_with(&expected_ref),
            "CLAUDE.md should end with @ reference, got: {claude_content}"
        );
    }

    #[tokio::test]
    async fn run_without_project_instruction() {
        let tmp = TempDir::new().unwrap();
        setup_strategy_file(&tmp, "code", "# Code Worker");

        run(claude(&tmp), SkillLayout::AgentHome, &strategy("code")).await;

        assert!(!tmp.path().join("home/.claude/PROJECT_CLAUDE.md").exists());
        assert_eq!(read_claude_md(&tmp), "# Code Worker");
    }

    #[tokio::test]
    async fn run_project_instruction_without_host_workspace() {
        let tmp = TempDir::new().unwrap();
        setup_strategy_file(&tmp, "code", "# Code Worker");
        let project_dir = tmp.path().join("project-instruction");
        std::fs::create_dir_all(&project_dir).unwrap();
        let project_path = project_dir.join("CLAUDE.md");
        std::fs::write(&project_path, "# Project\nPath: %WORKSPACE%/foo").unwrap();

        let inputs = with_project("code", &project_path, None, ProjectInstructionMode::Import);
        run(claude(&tmp), SkillLayout::AgentHome, &inputs).await;

        let project_dest = tmp.path().join("home/.claude/PROJECT_CLAUDE.md");
        assert_eq!(
            std::fs::read_to_string(&project_dest).unwrap(),
            "# Project\nPath: %WORKSPACE%/foo",
            "content should pass through unchanged without a host workspace"
        );
    }

    #[tokio::test]
    async fn missing_project_instruction_file_errors() {
        let tmp = TempDir::new().unwrap();
        setup_strategy_file(&tmp, "code", "# Code Worker");

        let inputs = with_project(
            "code",
            &tmp.path().join("absent.md"),
            None,
            ProjectInstructionMode::Import,
        );
        let result = InitInstructionsManager::new(claude(&tmp), SkillLayout::AgentHome)
            .run(&inputs)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn agy_uses_customization_root_and_folds_project_instructions() {
        let tmp = TempDir::new().unwrap();
        setup_strategy_file(&tmp, "code", "# Code Worker");
        std::fs::create_dir_all(tmp.path().join("home/.gemini/config")).unwrap();
        let project_path = tmp.path().join("CLAUDE.md");
        std::fs::write(&project_path, "# Project rules").unwrap();

        let inputs = with_project(
            "code",
            &project_path,
            None,
            ProjectInstructionMode::for_agent(AgentType::Agy),
        );
        run(
            temp_paths(&tmp, AgentType::Agy),
            SkillLayout::AgentHome,
            &inputs,
        )
        .await;

        assert_eq!(
            std::fs::read_to_string(tmp.path().join("home/.gemini/config/AGENTS.md")).unwrap(),
            "# Code Worker\n\n# Project rules"
        );
        assert!(
            !tmp.path()
                .join("home/.gemini/config/PROJECT_AGENTS.md")
                .exists()
        );
    }

    /// The mode is the caller's choice, independent of the agent's paths.
    #[tokio::test]
    async fn inline_mode_folds_project_instructions_for_claude_paths() {
        let tmp = TempDir::new().unwrap();
        setup_strategy_file(&tmp, "code", "# Code Worker");
        let project_path = tmp.path().join("proj.md");
        std::fs::write(&project_path, "ws=%WORKSPACE%").unwrap();

        let inputs = with_project(
            "code",
            &project_path,
            Some("/w"),
            ProjectInstructionMode::Inline,
        );
        run(claude(&tmp), SkillLayout::AgentHome, &inputs).await;

        assert_eq!(read_claude_md(&tmp), "# Code Worker\n\nws=/w");
        assert!(!tmp.path().join("home/.claude/PROJECT_CLAUDE.md").exists());
    }

    #[tokio::test]
    async fn run_with_gitea_forge_appends_section() {
        let tmp = TempDir::new().unwrap();
        setup_strategy_file(&tmp, "code", "# Code Worker");

        run(
            claude(&tmp),
            SkillLayout::AgentHome,
            &with_forge("code", "gitea"),
        )
        .await;

        let content = read_claude_md(&tmp);
        assert!(content.contains("# Forge Collaboration"));
        assert!(content.contains("Gitea"));
        assert!(content.contains("`tea`"));
        assert!(content.contains("do not use `gh`"));
    }

    #[tokio::test]
    async fn run_with_github_forge_appends_section() {
        let tmp = TempDir::new().unwrap();
        setup_strategy_file(&tmp, "code", "# Code Worker");

        run(
            claude(&tmp),
            SkillLayout::AgentHome,
            &with_forge("code", "github"),
        )
        .await;

        let content = read_claude_md(&tmp);
        assert!(content.contains("# Forge Collaboration"));
        assert!(content.contains("GitHub"));
        assert!(content.contains("`gh`"));
        assert!(content.contains("do not use `tea`"));
    }

    #[tokio::test]
    async fn run_with_unset_forge_skips_section() {
        let tmp = TempDir::new().unwrap();
        setup_strategy_file(&tmp, "code", "# Code Worker");

        run(claude(&tmp), SkillLayout::AgentHome, &strategy("code")).await;

        assert!(!read_claude_md(&tmp).contains("# Forge Collaboration"));
    }

    #[tokio::test]
    async fn run_with_empty_forge_skips_section() {
        let tmp = TempDir::new().unwrap();
        setup_strategy_file(&tmp, "code", "# Code Worker");

        run(
            claude(&tmp),
            SkillLayout::AgentHome,
            &with_forge("code", "   "),
        )
        .await;

        assert!(!read_claude_md(&tmp).contains("# Forge Collaboration"));
    }

    #[tokio::test]
    async fn run_with_gitea_forge_works_for_agy() {
        let tmp = TempDir::new().unwrap();
        setup_strategy_file(&tmp, "code", "# Code Worker");
        std::fs::create_dir_all(tmp.path().join("home/.gemini/config")).unwrap();

        run(
            temp_paths(&tmp, AgentType::Agy),
            SkillLayout::AgentHome,
            &with_forge("code", "gitea"),
        )
        .await;

        let content =
            std::fs::read_to_string(tmp.path().join("home/.gemini/config/AGENTS.md")).unwrap();
        assert!(content.contains("# Forge Collaboration"));
        assert!(content.contains("Gitea"));
        assert!(content.contains("`tea`"));
    }
}
