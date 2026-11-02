use std::collections::HashSet;
use std::path::Path;

use tracing::{info, warn};

use super::copy_dir_recursive;
use super::skill_render::render_skill_text;
use crate::paths::WorkerPaths;
use crate::skill_layout::SkillLayout;

const POTENTIAL_SKILLS_DIR: &str = "potential-skills";
const PLUGIN_MANIFEST_DIR: &str = ".claude-plugin";
const PLUGIN_MANIFEST_FILE: &str = "plugin.json";
const PLUGIN_DESCRIPTION: &str = "ur worker skills";

/// Installs a worker's selected skills from `potential-skills/`.
///
/// The injected [`SkillLayout`] decides where skills land and whether
/// cross-skill references are namespaced; every layout renders the
/// `%UR_SKILL_HOOKS%` placeholder from [`WorkerPaths::skill_hooks`].
#[derive(Clone)]
pub struct InitSkillsManager {
    paths: WorkerPaths,
    layout: SkillLayout,
}

impl InitSkillsManager {
    pub fn new(paths: WorkerPaths, layout: SkillLayout) -> Self {
        InitSkillsManager { paths, layout }
    }

    /// Install `skills` from `<agent_shared>/potential-skills/` into
    /// `skills_dest` per the layout. `skills_dest` is always wiped and
    /// recreated, even when `skills` is empty.
    pub async fn run(&self, skills: &[String]) -> Result<(), std::io::Error> {
        let skills_dest = &self.paths.skills_dest;
        let potential_dir = self.paths.agent_shared.join(POTENTIAL_SKILLS_DIR);

        // Always wipe and recreate skills dir
        if skills_dest.exists() {
            tokio::fs::remove_dir_all(skills_dest).await?;
            info!(path = %skills_dest.display(), "removed existing skills directory");
        }
        let skills_root = self.layout.skills_root(skills_dest);
        tokio::fs::create_dir_all(&skills_root).await?;
        info!(path = %skills_root.display(), "created skills directory");

        if let Some(namespace) = self.layout.namespace() {
            write_plugin_manifest(skills_dest, namespace).await?;
        }

        if skills.is_empty() {
            info!("no skills requested");
            return Ok(());
        }
        info!(?skills, "processing skill list");

        // Resolve the full installed set first: cross-skill rewriting must
        // know every skill that will exist, not just those copied so far.
        let mut installed: Vec<&str> = Vec::new();
        for name in skills.iter().map(|s| s.trim()) {
            if name.is_empty() {
                continue;
            }
            let src = potential_dir.join(name);
            if src.exists() {
                installed.push(name);
            } else {
                warn!(skill = name, path = %src.display(), "skill not found in potential-skills");
            }
        }
        let installed_set: HashSet<&str> = installed.iter().copied().collect();

        for name in &installed {
            let src = potential_dir.join(name);
            let dst = skills_root.join(name);
            copy_dir_recursive(&src, &dst).await?;
            self.render_tree(&dst, &installed_set).await?;
            info!(skill = name, src = %src.display(), dst = %dst.display(), "installed skill");
        }

        Ok(())
    }

    /// Render every UTF-8 file under `dir` in place. Binary files and files the
    /// render leaves unchanged are not rewritten.
    async fn render_tree(&self, dir: &Path, installed: &HashSet<&str>) -> std::io::Result<()> {
        let mut entries = tokio::fs::read_dir(dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if entry.file_type().await?.is_dir() {
                Box::pin(self.render_tree(&path, installed)).await?;
                continue;
            }
            let bytes = tokio::fs::read(&path).await?;
            let Ok(text) = String::from_utf8(bytes) else {
                continue;
            };
            let rendered =
                render_skill_text(&text, &self.layout, installed, &self.paths.skill_hooks);
            if rendered != text {
                tokio::fs::write(&path, rendered).await?;
            }
        }
        Ok(())
    }
}

/// Write `<plugin_root>/.claude-plugin/plugin.json` for a namespaced plugin.
async fn write_plugin_manifest(plugin_root: &Path, namespace: &str) -> std::io::Result<()> {
    let manifest_dir = plugin_root.join(PLUGIN_MANIFEST_DIR);
    tokio::fs::create_dir_all(&manifest_dir).await?;
    let manifest = serde_json::json!({
        "name": namespace,
        "version": env!("CARGO_PKG_VERSION"),
        "description": PLUGIN_DESCRIPTION,
    });
    let body = serde_json::to_string_pretty(&manifest).map_err(std::io::Error::other)?;
    let path = manifest_dir.join(PLUGIN_MANIFEST_FILE);
    tokio::fs::write(&path, body).await?;
    info!(path = %path.display(), "wrote plugin manifest");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init::skill_render::SKILL_HOOKS_PLACEHOLDER;
    use crate::test_support::{container_paths_with_home, temp_paths};
    use std::path::PathBuf;
    use tempfile::TempDir;
    use ur_config::AgentType;

    const CONTAINER_SKILL_HOOKS: &str = "/home/worker/.claude/skill-hooks";

    fn write_skill(paths: &WorkerPaths, name: &str, file: &str, content: &str) {
        let dir = paths.agent_shared.join(POTENTIAL_SKILLS_DIR).join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(file), content).unwrap();
    }

    async fn run_with_skills(paths: &WorkerPaths, layout: SkillLayout, skills: &[&str]) {
        let skills: Vec<String> = skills.iter().map(|s| (*s).to_owned()).collect();
        InitSkillsManager::new(paths.clone(), layout)
            .run(&skills)
            .await
            .unwrap();
    }

    fn repo_potential_skills() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../containers/worker-base")
            .join(POTENTIAL_SKILLS_DIR)
    }

    /// Every tracked-style skill dir in the repo (skips anything without SKILL.md).
    fn repo_skill_names() -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(repo_potential_skills())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().join("SKILL.md").exists())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn walk_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk_files(&path, out);
            } else {
                out.push(path);
            }
        }
    }

    /// Assert `out` is `src` with only `%UR_SKILL_HOOKS%` rendered to the old
    /// container literal (binary files byte-identical). Returns whether the
    /// output references the container hook path.
    fn assert_container_rendered(src: &Path, out: &Path) -> bool {
        let src_bytes = std::fs::read(src).unwrap();
        let out_bytes = std::fs::read(out).unwrap();
        let Ok(text) = String::from_utf8(src_bytes.clone()) else {
            assert_eq!(out_bytes, src_bytes, "{}", out.display());
            return false;
        };
        let rendered = String::from_utf8(out_bytes).unwrap();
        assert!(
            !rendered.contains(SKILL_HOOKS_PLACEHOLDER),
            "{}",
            out.display()
        );
        assert_eq!(
            rendered,
            text.replace(SKILL_HOOKS_PLACEHOLDER, CONTAINER_SKILL_HOOKS),
            "{}",
            out.display()
        );
        rendered.contains("@/home/worker/.claude/skill-hooks/")
    }

    #[tokio::test]
    async fn run_copies_named_skills() {
        let tmp = TempDir::new().unwrap();
        let paths = temp_paths(&tmp, AgentType::Claude);
        write_skill(&paths, "my-skill", "SKILL.md", "# My Skill");

        run_with_skills(&paths, SkillLayout::AgentHome, &["my-skill"]).await;

        let dest = paths.skills_dest.join("my-skill/SKILL.md");
        assert!(
            dest.starts_with(tmp.path()),
            "output must land under temp root"
        );
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "# My Skill");
        assert!(!paths.skills_dest.join(PLUGIN_MANIFEST_DIR).exists());
    }

    #[tokio::test]
    async fn run_missing_skill_warns_but_succeeds() {
        let tmp = TempDir::new().unwrap();
        let paths = temp_paths(&tmp, AgentType::Claude);

        run_with_skills(&paths, SkillLayout::AgentHome, &["nonexistent"]).await;

        assert!(!paths.skills_dest.join("nonexistent").exists());
    }

    #[tokio::test]
    async fn run_no_skills_leaves_empty_skills_dir() {
        let tmp = TempDir::new().unwrap();

        let mgr =
            InitSkillsManager::new(temp_paths(&tmp, AgentType::Claude), SkillLayout::AgentHome);
        let result = mgr.run(&[]).await;

        assert!(result.is_ok(), "no skills should not cause an error");
        assert!(
            temp_paths(&tmp, AgentType::Claude).skills_dest.exists(),
            "skills dir is always (re)created"
        );
    }

    #[tokio::test]
    async fn run_wipes_existing_skills_dir_first() {
        let tmp = TempDir::new().unwrap();
        let skills_dir = temp_paths(&tmp, AgentType::Claude).skills_dest;
        std::fs::create_dir_all(skills_dir.join("stale-skill")).unwrap();
        std::fs::write(skills_dir.join("stale-skill/SKILL.md"), "old").unwrap();

        let mgr =
            InitSkillsManager::new(temp_paths(&tmp, AgentType::Claude), SkillLayout::AgentHome);
        mgr.run(&[]).await.unwrap();

        assert!(
            !skills_dir.join("stale-skill").exists(),
            "stale skill should be wiped"
        );
    }

    #[tokio::test]
    async fn agy_uses_customization_root() {
        let tmp = TempDir::new().unwrap();

        InitSkillsManager::new(temp_paths(&tmp, AgentType::Agy), SkillLayout::AgentHome)
            .run(&[])
            .await
            .unwrap();

        assert!(tmp.path().join("home/.gemini/config/skills").exists());
        assert!(
            !tmp.path()
                .join("home/.gemini/antigravity-cli/skills")
                .exists()
        );
    }

    #[tokio::test]
    async fn agent_home_renders_hooks_from_injected_path() {
        let tmp = TempDir::new().unwrap();
        let paths = temp_paths(&tmp, AgentType::Claude);
        write_skill(
            &paths,
            "implement",
            "SKILL.md",
            "Run /code-review.\n@%UR_SKILL_HOOKS%/implement/style-guide.md\n",
        );
        write_skill(&paths, "code-review", "SKILL.md", "# Review");

        run_with_skills(
            &paths,
            SkillLayout::AgentHome,
            &["implement", "code-review"],
        )
        .await;

        let out = std::fs::read_to_string(paths.skills_dest.join("implement/SKILL.md")).unwrap();
        assert_eq!(
            out,
            format!(
                "Run /code-review.\n@{}/implement/style-guide.md\n",
                paths.skill_hooks.display()
            )
        );
    }

    /// Container install of the real potential-skills: identical to the
    /// sources except `%UR_SKILL_HOOKS%`, which renders to the old literal.
    #[tokio::test]
    async fn container_install_of_repo_skills_matches_old_literal_output() {
        let tmp = TempDir::new().unwrap();
        let mut paths = temp_paths(&tmp, AgentType::Claude);
        paths.agent_shared = repo_potential_skills().parent().unwrap().to_path_buf();
        paths.skill_hooks = PathBuf::from(CONTAINER_SKILL_HOOKS);
        let names = repo_skill_names();
        assert!(names.iter().any(|n| n == "implement"));
        let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();

        run_with_skills(&paths, SkillLayout::AgentHome, &name_refs).await;

        let mut files = Vec::new();
        for name in &names {
            walk_files(&repo_potential_skills().join(name), &mut files);
        }
        let mut saw_hook_ref = false;
        for src in files {
            let rel = src.strip_prefix(repo_potential_skills()).unwrap();
            saw_hook_ref |= assert_container_rendered(&src, &paths.skills_dest.join(rel));
        }
        assert!(
            saw_hook_ref,
            "implement must reference the container hook path"
        );
    }

    /// Container skills render `%UR_SKILL_HOOKS%` to the agent's own
    /// skill-hooks dir, where `InitSkillHooksManager` copies the hooks.
    async fn assert_container_hook_render(agent: AgentType, expected: &str) {
        let tmp = TempDir::new().unwrap();
        let mut paths = temp_paths(&tmp, agent);
        paths.skill_hooks = container_paths_with_home(agent, "/home/worker")
            .await
            .skill_hooks;
        write_skill(
            &paths,
            "implement",
            "SKILL.md",
            "@%UR_SKILL_HOOKS%/implement/style-guide.md\n",
        );

        run_with_skills(&paths, SkillLayout::AgentHome, &["implement"]).await;

        let out = std::fs::read_to_string(paths.skills_dest.join("implement/SKILL.md")).unwrap();
        assert_eq!(out, format!("@{expected}/implement/style-guide.md\n"));
    }

    #[tokio::test]
    async fn codex_container_renders_codex_skill_hooks() {
        assert_container_hook_render(AgentType::Codex, "/home/worker/.codex/skill-hooks").await;
    }

    #[tokio::test]
    async fn agy_container_renders_gemini_config_skill_hooks() {
        assert_container_hook_render(AgentType::Agy, "/home/worker/.gemini/config/skill-hooks")
            .await;
    }

    #[tokio::test]
    async fn plugin_layout_writes_manifest_and_nested_skills() {
        let tmp = TempDir::new().unwrap();
        let paths = temp_paths(&tmp, AgentType::Claude);
        write_skill(
            &paths,
            "implement",
            "SKILL.md",
            "Run /code-review, then /ship.\nUse the `code-review` skill, not the `ship` skill.\n\
             @%UR_SKILL_HOOKS%/implement/a.md\n",
        );
        write_skill(
            &paths,
            "code-review",
            "SKILL.md",
            "See the `implement` skill.",
        );

        run_with_skills(
            &paths,
            SkillLayout::ur_plugin(),
            &["implement", "code-review"],
        )
        .await;

        let manifest: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(paths.skills_dest.join(".claude-plugin/plugin.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            manifest,
            serde_json::json!({
                "name": "ur",
                "version": env!("CARGO_PKG_VERSION"),
                "description": "ur worker skills",
            })
        );

        let implement =
            std::fs::read_to_string(paths.skills_dest.join("skills/implement/SKILL.md")).unwrap();
        assert_eq!(
            implement,
            format!(
                "Run /ur:code-review, then /ship.\nUse the `ur:code-review` skill, not the `ship` skill.\n\
                 @{}/implement/a.md\n",
                paths.skill_hooks.display()
            )
        );
        let review =
            std::fs::read_to_string(paths.skills_dest.join("skills/code-review/SKILL.md")).unwrap();
        assert_eq!(review, "See the `ur:implement` skill.");
        assert!(!paths.skills_dest.join("implement").exists());
    }

    #[tokio::test]
    async fn plugin_install_of_repo_skills_leaves_no_placeholder() {
        let tmp = TempDir::new().unwrap();
        let mut paths = temp_paths(&tmp, AgentType::Claude);
        paths.agent_shared = repo_potential_skills().parent().unwrap().to_path_buf();
        let names = repo_skill_names();
        let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();

        run_with_skills(&paths, SkillLayout::ur_plugin(), &name_refs).await;

        let mut files = Vec::new();
        walk_files(&paths.skills_dest, &mut files);
        assert!(files.len() > names.len());
        let leftovers: Vec<&PathBuf> = files
            .iter()
            .filter(|f| {
                std::fs::read_to_string(f).is_ok_and(|t| t.contains(SKILL_HOOKS_PLACEHOLDER))
            })
            .collect();
        assert!(
            leftovers.is_empty(),
            "placeholder survived in {leftovers:?}"
        );
        let implement =
            std::fs::read_to_string(paths.skills_dest.join("skills/implement/SKILL.md")).unwrap();
        assert!(implement.contains(&format!("@{}/implement/", paths.skill_hooks.display())));
    }
}
