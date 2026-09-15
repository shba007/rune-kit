pub mod manifest;
pub mod package;
pub mod protocol;
pub mod runtime;

pub use manifest::{
    ExecutionKind, InstalledPlugin, Lockfile, PluginUpdateStatus, PromptArgument,
    PromptDefinition, RegistryPluginSummary, ResourceDefinition, ToolDefinition,
};
pub use package::{PackageError, PackageManager, is_newer_version};
pub use protocol::McpRouter;
pub use runtime::{NativeSidecar, PluginInstance, RuntimeError, WasmPluginInstance};
