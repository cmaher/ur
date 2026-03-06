use anyhow::Result;

use crate::{BuildOpts, ContainerId, ContainerRuntime, ImageId, RunOpts};

pub struct DockerRuntime;

impl ContainerRuntime for DockerRuntime {
    fn build(&self, _opts: &BuildOpts) -> Result<ImageId> {
        todo!()
    }
    fn run(&self, _opts: &RunOpts) -> Result<ContainerId> {
        todo!()
    }
    fn stop(&self, _id: &ContainerId) -> Result<()> {
        todo!()
    }
    fn rm(&self, _id: &ContainerId) -> Result<()> {
        todo!()
    }
}
