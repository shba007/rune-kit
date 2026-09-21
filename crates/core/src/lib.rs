pub mod manifest;
pub mod package;
pub mod protocol;
pub mod provision;
pub mod runtime;

pub use manifest::{
    BinaryDependencySpec, Capabilities, ExecCapability, ExecutionKind, FilesystemCapability,
    InstalledPlugin, InstalledSkill, Lockfile, PluginInfo, PluginManifest, PluginUpdateStatus,
    PromptArgument, PromptDefinition, RegistryPluginSummary, RegistrySkillSummary,
    ResourceDefinition, SkillFile, SkillLockfile, SkillRow, ToolDefinition,
};
pub use package::{PackageError, PackageManager, SkillManager, is_newer_version};
pub use protocol::McpRouter;
pub use provision::{BinaryProvisioner, ProvisionError};
pub use runtime::{NativeSidecar, PluginInstance, RuntimeError, WasmPluginInstance};
