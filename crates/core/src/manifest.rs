use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
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
pub struct PluginManifest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub capabilities: Capabilities,
    #[serde(default)]
    pub dependencies: Dependencies,
    #[serde(default)]
    pub tools: HashMap<String, ToolRuleSpec>,
}

impl PluginManifest {
    pub fn from_toml(content: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(content)
    }

    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, std::io::Error> {
        let content = std::fs::read_to_string(path)?;
        Self::from_toml(&content)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Capabilities {
    #[serde(default)]
    pub network_hosts: Vec<String>,
    #[serde(default)]
    pub filesystem: Option<FilesystemCapability>,
    #[serde(default)]
    pub exec: ExecCapability,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FilesystemCapability {
    #[serde(default = "default_scoped_mode")]
    pub mode: String,
    #[serde(default)]
    pub root_param: Option<String>,
}

fn default_scoped_mode() -> String {
    "scoped".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExecCapability {
    #[serde(default)]
    pub allowed_binaries: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Dependencies {
    #[serde(default)]
    pub binaries: HashMap<String, BinaryDependencySpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BinaryDependencySpec {
    pub version: String,
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub sha256: Option<String>,
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
    #[serde(default)]
    pub manifest: Option<PluginManifest>,
}

/// Canonical MCP tool facts. Both wire spellings are accepted, including equal pairs.
/// tools/list retains legacy output throughout 0.3.x; removal requires a minor-version
/// release and an explicit migration note. Legacy input remains supported.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
pub struct ToolAnnotations {
    #[serde(rename = "readOnlyHint", skip_serializing_if = "Option::is_none")]
    pub read_only_hint: Option<bool>,
    #[serde(rename = "destructiveHint", skip_serializing_if = "Option::is_none")]
    pub destructive_hint: Option<bool>,
    #[serde(rename = "idempotentHint", skip_serializing_if = "Option::is_none")]
    pub idempotent_hint: Option<bool>,
    #[serde(rename = "openWorldHint", skip_serializing_if = "Option::is_none")]
    pub open_world_hint: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

// A present fact must be a boolean. Null is not a valid declaration and must not
// fall through to a permissive value in the other spelling or manifest defaults.
fn annotation_bool<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<bool>, D::Error> {
    bool::deserialize(deserializer).map(Some)
}

impl<'de> Deserialize<'de> for ToolAnnotations {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Input {
            #[serde(default, deserialize_with = "annotation_bool")]
            read_only_hint: Option<bool>,
            #[serde(default, deserialize_with = "annotation_bool")]
            read_only: Option<bool>,
            #[serde(default, deserialize_with = "annotation_bool")]
            destructive_hint: Option<bool>,
            #[serde(default, deserialize_with = "annotation_bool")]
            destructive: Option<bool>,
            #[serde(default, deserialize_with = "annotation_bool")]
            idempotent_hint: Option<bool>,
            #[serde(default, deserialize_with = "annotation_bool")]
            idempotent: Option<bool>,
            #[serde(default, deserialize_with = "annotation_bool")]
            open_world_hint: Option<bool>,
            #[serde(default, deserialize_with = "annotation_bool")]
            open_world: Option<bool>,
            #[serde(default)]
            title: Option<String>,
        }
        fn merge<E: serde::de::Error>(
            canonical: Option<bool>,
            legacy: Option<bool>,
            canonical_name: &str,
            legacy_name: &str,
        ) -> Result<Option<bool>, E> {
            match (canonical, legacy) {
                (Some(a), Some(b)) if a != b => Err(E::custom(format!(
                    "conflicting annotation values for {canonical_name} and {legacy_name}"
                ))),
                _ => Ok(canonical.or(legacy)),
            }
        }
        let input = Input::deserialize(deserializer)?;
        Ok(Self {
            read_only_hint: merge(
                input.read_only_hint,
                input.read_only,
                "readOnlyHint",
                "readOnly",
            )?,
            destructive_hint: merge(
                input.destructive_hint,
                input.destructive,
                "destructiveHint",
                "destructive",
            )?,
            idempotent_hint: merge(
                input.idempotent_hint,
                input.idempotent,
                "idempotentHint",
                "idempotent",
            )?,
            open_world_hint: merge(
                input.open_world_hint,
                input.open_world,
                "openWorldHint",
                "openWorld",
            )?,
            title: input.title,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ToolRuleSpec {
    #[serde(default)]
    pub read_only: Option<bool>,
    #[serde(default)]
    pub destructive: Option<bool>,
    #[serde(default)]
    pub idempotent: Option<bool>,
    #[serde(default)]
    pub open_world: Option<bool>,
    #[serde(default)]
    pub description: Option<String>,
}

impl From<ToolRuleSpec> for ToolAnnotations {
    fn from(rule: ToolRuleSpec) -> Self {
        Self {
            read_only_hint: rule.read_only,
            destructive_hint: rule.destructive,
            idempotent_hint: rule.idempotent,
            open_world_hint: rule.open_world,
            title: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema", alias = "input_schema")]
    pub input_schema: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<ToolAnnotations>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceDefinition {
    pub uri: String,
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
pub struct RegistrySkillSummary {
    pub name: String,
    pub latest: String,
    pub description: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

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

pub trait SkillRow {
    fn name(&self) -> &str;
    fn version(&self) -> &str;
    fn description(&self) -> Option<&str>;
    fn author(&self) -> Option<&str>;
}

impl SkillRow for InstalledSkill {
    fn name(&self) -> &str {
        &self.name
    }
    fn version(&self) -> &str {
        &self.version
    }
    fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }
    fn author(&self) -> Option<&str> {
        self.author.as_deref()
    }
}

impl SkillRow for RegistrySkillSummary {
    fn name(&self) -> &str {
        &self.name
    }
    fn version(&self) -> &str {
        &self.latest
    }
    fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }
    fn author(&self) -> Option<&str> {
        self.author.as_deref()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillFile {
    pub path: String,
    #[serde(default)]
    pub sha256: Option<String>,
}

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
