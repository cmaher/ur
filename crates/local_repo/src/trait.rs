use anyhow::Result;
use async_trait::async_trait;

use crate::types::{HookResult, PushResult};

#[async_trait]
pub trait LocalRepo: Send + Sync {
    // Push operations
    async fn push(&self, branch: &str, working_dir: &str) -> Result<PushResult>;
    async fn force_push(&self, branch: &str, working_dir: &str) -> Result<PushResult>;

    // Hook execution
    async fn run_hook(&self, script_path: &str, working_dir: &str) -> Result<HookResult>;

    // Pool operations
    async fn clone(&self, url: &str, path: &str, parent_dir: &str) -> Result<()>;
    async fn fetch(&self, working_dir: &str) -> Result<()>;
    async fn reset_hard(&self, working_dir: &str, ref_name: &str) -> Result<()>;
    async fn clean(&self, working_dir: &str) -> Result<()>;
    async fn checkout_branch(&self, working_dir: &str, branch: &str) -> Result<()>;
    async fn submodule_update(&self, working_dir: &str) -> Result<()>;
}
