use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionKind {
    Wasm,
    Native,
}

impl Default for ExecutionKind {
    fn default() -> Self {
        ExecutionKind::Wasm
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PluginInfo {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Lockfile {
    pub version: u32,
    pub plugins: HashMap<String, InstalledPlugin>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledPlugin {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub version: String,
    pub binary_path: String,
    #[serde(default)]
    pub native_binary_path: Option<String>,
    #[serde(default)]
    pub execution_kind: ExecutionKind,
    pub sha256: String,
    pub source: String,
    pub default_params: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema", alias = "input_schema")]
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceDefinition {
    pub uri: String, // plugin-local, unqualified — rune-kit prefixes with rune://<namespace>/
    pub name: String,
    pub description: String,
    #[serde(rename = "mimeType", alias = "mime_type", default)]
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptArgument {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptDefinition {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub arguments: Vec<PromptArgument>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryPluginSummary {
    pub name: String,
    pub latest: String,
    pub description: Option<String>,
    pub has_wasm: bool,
    pub has_native: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginUpdateStatus {
    pub name: String,
    pub installed_version: String,
    pub latest_version: String,
    pub has_update: bool,
    pub has_wasm: bool,
    pub has_native: bool,
}
