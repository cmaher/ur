pub mod config;
pub mod discovery;
pub mod error;
pub mod forward;
pub mod lua_transform;
pub mod paths;
pub mod script_registry;
pub mod shim;

pub use config::HostExecConfigManager;
pub use discovery::LuaDiscoveryManager;
pub use error::HostExecError;
pub use forward::{CommandOutputStream, forward_to_builderd};
pub use lua_transform::{LuaCommandMetadata, LuaTransformManager, TransformResult, WorkerContext};
pub use paths::{
    BRAIN_HOSTEXEC_COMMAND, is_brain_dir, map_brain_working_dir_impl, map_script_path_impl,
    map_working_dir_impl, validate_script_path,
};
pub use script_registry::ScriptRegistry;
pub use shim::materialize_shim;
