// crates/rune-kit-core/src/lib.rs
pub mod manifest;
pub mod package;
pub mod protocol;
pub mod runtime;

pub use manifest::{InstalledPlugin, Lockfile, ToolDefinition};
pub use package::{PackageError, PackageManager};
pub use protocol::McpRouter;
pub use runtime::{RuntimeError, WasmPluginInstance};
