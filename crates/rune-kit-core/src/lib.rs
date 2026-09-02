pub mod manifest;
pub mod package;
pub mod protocol;
pub mod runtime;

pub use manifest::{ExecutionKind, InstalledPlugin, Lockfile, ToolDefinition};
pub use package::{PackageError, PackageManager};
pub use protocol::McpRouter;
pub use runtime::{NativeSidecar, PluginInstance, RuntimeError, WasmPluginInstance};
