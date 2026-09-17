use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ExecutionKind {
    #[default]
    Wasm,
    Native,
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

// A registry entry for a skill — a standalone published content artifact,
// kept separate from the MCP plugin registry (its own `SkillLockfile`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistrySkillSummary {
    pub name: String,
    pub latest: String,
    pub description: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

// A skill's on-disk lockfile entry: installed content (SKILL.md + files),
// not executable code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledSkill {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub version: String,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub files: Vec<SkillFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillFile {
    pub path: String,
    #[serde(default)]
    pub sha256: Option<String>,
}

// A skill lockfile, separate from `Lockfile` — skills and plugins stay separate.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SkillLockfile {
    pub version: u32,
    #[serde(default)]
    pub skills: HashMap<String, InstalledSkill>,
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
