//! Versioned registry configuration and artifact integrity metadata.
use crate::package::PackageError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    pub schema_version: u32,
    pub fields: BTreeMap<String, ConfigurationField>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationField {
    #[serde(rename = "type")]
    pub kind: String,
    pub description: String,
    pub required: bool,
    pub secret: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ArtifactChecksum {
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VersionMetadata {
    pub configuration: Configuration,
    pub checksums: BTreeMap<String, ArtifactChecksum>,
}

fn invalid(message: impl Into<String>) -> PackageError {
    PackageError::InvalidSpec(message.into())
}

impl VersionMetadata {
    pub fn from_value(value: &Value) -> Result<Self, PackageError> {
        let metadata: Self = serde_json::from_value(value.clone()).map_err(|e| {
            invalid(format!(
                "Missing or invalid versioned registry metadata: {e}"
            ))
        })?;
        metadata.configuration.validate()?;
        for checksum in metadata.checksums.values() {
            if checksum.size == 0
                || checksum.sha256.len() != 64
                || !checksum.sha256.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err(invalid("Invalid plugin artifact checksum or size"));
            }
        }
        Ok(metadata)
    }

    pub fn checksum(&self, url: &str) -> Result<&ArtifactChecksum, PackageError> {
        let name = url.rsplit('/').next().unwrap_or_default();
        self.checksums
            .get(name)
            .ok_or_else(|| invalid(format!("Missing checksum for plugin artifact {name}")))
    }
}

impl Configuration {
    pub fn validate(&self) -> Result<(), PackageError> {
        if self.schema_version != 1 {
            return Err(invalid("Unsupported plugin configuration schema_version"));
        }
        for (key, field) in &self.fields {
            let valid_default = field.default.as_ref().is_none_or(|value| {
                !field.secret
                    && match field.kind.as_str() {
                        "string" => value.is_string(),
                        "integer" => value.is_i64() || value.is_u64(),
                        "number" => value.is_number(),
                        "boolean" => value.is_boolean(),
                        _ => false,
                    }
            });
            if key.is_empty()
                || !key
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                || field.description.trim().is_empty()
                || !matches!(
                    field.kind.as_str(),
                    "string" | "integer" | "number" | "boolean"
                )
                || (field.env.is_empty() && field.config_key.is_none())
                || field.env.iter().any(|name| {
                    name.is_empty()
                        || !name
                            .chars()
                            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
                })
                || field
                    .config_key
                    .as_ref()
                    .is_some_and(|name| name.trim().is_empty())
                || !valid_default
            {
                return Err(invalid(format!("Invalid plugin configuration field {key}")));
            }
        }
        Ok(())
    }
}

impl ArtifactChecksum {
    pub fn verify(&self, bytes: &[u8]) -> Result<(), PackageError> {
        let actual = Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        if self.size != bytes.len() as u64 || !actual.eq_ignore_ascii_case(&self.sha256) {
            return Err(invalid(
                "Plugin artifact checksum or size mismatch; installation was not changed",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn metadata() -> Value {
        json!({
            "configuration": {"schema_version": 1, "fields": {
                "api_key": {"type": "string", "description": "Service credential",
                    "required": true, "secret": true, "env": ["API_KEY"]},
                "timezone": {"type": "string", "description": "Default timezone",
                    "required": false, "secret": false, "config_key": "default_timezone",
                    "default": "UTC"}
            }},
            "checksums": {"plugin.wasm": {"sha256": Sha256::digest(b"plugin").iter().map(|b| format!("{b:02x}")).collect::<String>(), "size": 6}},
            "url": "https://example.test/plugin.wasm"
        })
    }

    #[test]
    fn version_metadata_preserves_native_and_wasm_configuration_in_installed_manifest() {
        let parsed = VersionMetadata::from_value(&metadata()).unwrap();
        parsed
            .checksum("https://example.test/plugin.wasm")
            .unwrap()
            .verify(b"plugin")
            .unwrap();
        let manifest = crate::manifest::PluginManifest {
            configuration: Some(parsed.configuration),
            ..Default::default()
        };
        let encoded = toml::to_string(&manifest).unwrap();
        let restored = crate::manifest::PluginManifest::from_toml(&encoded).unwrap();
        let config = restored.configuration.unwrap();
        assert!(config.fields["api_key"].secret);
        assert_eq!(config.fields["api_key"].env, ["API_KEY"]);
        assert_eq!(config.fields["timezone"].default, Some(json!("UTC")));
        assert!(
            crate::manifest::PluginManifest::from_toml("")
                .unwrap()
                .configuration
                .is_none()
        );
    }

    #[test]
    fn malformed_or_missing_metadata_is_rejected() {
        for (path, value) in [
            (vec!["configuration", "schema_version"], json!(2)),
            (
                vec!["configuration", "fields", "api_key", "default"],
                json!("secret"),
            ),
            (
                vec!["configuration", "fields", "timezone", "default"],
                json!(12),
            ),
            (vec!["configuration", "fields", "api_key", "env"], json!([])),
            (
                vec!["configuration", "fields", "api_key", "env"],
                json!(["bad=name"]),
            ),
            (
                vec!["configuration", "fields", "api_key", "type"],
                json!("object"),
            ),
            (vec!["checksums", "plugin.wasm", "sha256"], json!("bad")),
            (vec!["checksums", "plugin.wasm", "size"], json!(0)),
        ] {
            let mut fixture = metadata();
            let mut target = &mut fixture;
            for key in path {
                target = &mut target[key];
            }
            *target = value;
            assert!(VersionMetadata::from_value(&fixture).is_err());
        }
        assert!(VersionMetadata::from_value(&json!({"url":"legacy.wasm"})).is_err());
    }

    #[test]
    fn missing_checksum_corruption_and_truncation_are_rejected() {
        let parsed = VersionMetadata::from_value(&metadata()).unwrap();
        assert!(parsed.checksum("https://example.test/other.wasm").is_err());
        let checksum = parsed.checksum("https://example.test/plugin.wasm").unwrap();
        assert!(checksum.verify(b"plugi").is_err());
        assert!(checksum.verify(b"broken").is_err());
        assert!(checksum.verify(b"").is_err());
    }
}
