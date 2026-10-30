//! Shared test helpers for workerd unit tests.

use std::path::Path;

use tempfile::TempDir;
use tokio::sync::Mutex;
use ur_config::AgentType;

use crate::paths::WorkerPaths;

/// Serializes every test in this crate that mutates process environment variables.
pub static ENV_LOCK: Mutex<()> = Mutex::const_new(());

/// Build `WorkerPaths` for `agent` rooted entirely under `tmp`, mirroring the
/// container layout: `<tmp>/workspace`, `<tmp>/home`, `<tmp>/host-hooks`.
pub fn temp_paths(tmp: &TempDir, agent: AgentType) -> WorkerPaths {
    rooted_paths(tmp.path(), agent)
}

fn rooted_paths(root: &Path, agent: AgentType) -> WorkerPaths {
    let home = root.join("home");
    let customization_root = home.join(agent.customization_root());
    let settings_dir = home.join(agent.home_subdir());
    WorkerPaths {
        workspace: root.join("workspace"),
        skill_hooks: customization_root.join(agent.skill_hooks_subdir()),
        skills_dest: customization_root.join(agent.skill_subdir()),
        instructions_dest: customization_root.join(agent.instruction_filename()),
        settings_template: agent
            .settings_filename()
            .map(|_| settings_dir.join("potential-settings.json")),
        settings_dest: agent
            .settings_filename()
            .map(|filename| settings_dir.join(filename)),
        host_hooks: root.join("host-hooks"),
        agent_shared: home.join(".agent-shared"),
        shim_dir: home.join(".local/bin"),
        agent_home: home,
    }
}
