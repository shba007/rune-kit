pub mod manifest;
pub mod package;
pub mod protocol;
pub mod runtime;

pub use manifest::{
    ExecutionKind, InstalledPlugin, InstalledSkill, Lockfile, PluginUpdateStatus, PromptArgument,
    PromptDefinition, RegistryPluginSummary, RegistrySkillSummary, ResourceDefinition, SkillFile,
    SkillLockfile, ToolDefinition,
};
pub use package::{PackageError, PackageManager, SkillManager, is_newer_version};
pub use protocol::McpRouter;
pub use runtime::{NativeSidecar, PluginInstance, RuntimeError, WasmPluginInstance};
