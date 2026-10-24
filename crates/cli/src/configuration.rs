//! Dashboard edits replace only this server's plugin instances, between requests.
use rune_kit_core::registry::ConfigurationField;
use rune_kit_core::{
    ExecutionKind, McpRouter, NativeSidecar, PluginInstance, PluginManifest, WasmPluginInstance,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    sync::{Arc, Mutex},
};

#[derive(Clone)]
pub struct Plugin {
    pub name: String,
    pub path: PathBuf,
    pub kind: ExecutionKind,
    pub manifest: PluginManifest,
    pub params: HashMap<String, String>,
}

impl Plugin {
    fn parameters(&self, settings: &Settings) -> Result<HashMap<String, String>, String> {
        let mut params = self.params.clone();
        if let Some(config) = &self.manifest.configuration {
            config
                .validate()
                .map_err(|_| "Invalid configuration metadata")?;
            for (key, field) in &config.fields {
                if let Some(value) = settings
                    .plugins
                    .get(&self.name)
                    .and_then(|p| p.get(key))
                    .or_else(|| settings.shared.get(key))
                {
                    validate(field, value)?;
                    if locked(key, field, &self.manifest) {
                        return Err("Filesystem settings require sandbox support".into());
                    }
                    match self.kind {
                        ExecutionKind::Wasm => {
                            if let Some(key) = &field.config_key {
                                params.insert(key.clone(), value.clone());
                            }
                        }
                        ExecutionKind::Native => {
                            for key in &field.env {
                                params.insert(key.clone(), value.clone());
                            }
                        }
                    }
                }
            }
        }
        Ok(params)
    }

    pub fn load(&self, settings: &Settings) -> Result<PluginInstance, String> {
        let params = self.parameters(settings)?;
        match self.kind {
            ExecutionKind::Wasm => {
                let bytes = std::fs::read(&self.path).map_err(|_| "Cannot read plugin payload")?;
                WasmPluginInstance::load_from_bytes(
                    &self.name,
                    bytes,
                    params,
                    self.manifest.clone(),
                )
                .map(|p| PluginInstance::Wasm(Box::new(p)))
            }
            ExecutionKind::Native => {
                NativeSidecar::new(&self.name, &self.path, params, Some(self.manifest.clone()))
                    .map(|p| PluginInstance::Native(Box::new(p)))
            }
        }
        .map_err(|_| "Cannot initialize plugin; previous configuration retained".into())
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub shared: BTreeMap<String, String>,
    pub plugins: BTreeMap<String, BTreeMap<String, String>>,
}

pub struct Configuration {
    pub plugins: Vec<Plugin>,
    settings: Settings,
    path: PathBuf,
    original: Option<Vec<u8>>,
}

struct SaveLock(PathBuf);
impl Drop for SaveLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[derive(Clone)]
pub struct Field {
    pub plugin: String,
    pub key: String,
    pub schema: ConfigurationField,
    pub value: String,
    pub locked: bool,
}

pub fn locked(key: &str, field: &ConfigurationField, manifest: &PluginManifest) -> bool {
    key == "allowed_dir"
        || field.config_key.as_deref() == Some("allowed_dir")
        || field.env.iter().any(|k| k == "ALLOWED_DIR")
        || manifest
            .capabilities
            .filesystem
            .as_ref()
            .and_then(|f| f.root_param.as_deref())
            .is_some_and(|root| root == key)
}

pub fn validate(field: &ConfigurationField, value: &str) -> Result<(), String> {
    let valid = match field.kind.as_str() {
        "string" => !field.required || !value.trim().is_empty(),
        "integer" => value.parse::<i64>().is_ok() || value.parse::<u64>().is_ok(),
        "number" => value.parse::<f64>().is_ok_and(f64::is_finite),
        "boolean" => matches!(value, "true" | "false"),
        _ => false,
    };
    if valid && !value.chars().any(char::is_control) {
        Ok(())
    } else {
        Err(format!("Enter a valid {} value", field.kind))
    }
}

impl Configuration {
    pub fn read(path: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        let original = match std::fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        let settings = original
            .as_ref()
            .map(|bytes| serde_json::from_slice(bytes))
            .transpose()?
            .unwrap_or_default();
        Ok(Self {
            plugins: Vec::new(),
            settings,
            path,
            original,
        })
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn fields(&self) -> Vec<Field> {
        let mut fields = Vec::new();
        for plugin in &self.plugins {
            if let Some(config) = &plugin.manifest.configuration {
                for (key, schema) in &config.fields {
                    let value = self
                        .settings
                        .plugins
                        .get(&plugin.name)
                        .and_then(|p| p.get(key))
                        .or_else(|| self.settings.shared.get(key))
                        .cloned()
                        .or_else(|| {
                            schema
                                .config_key
                                .as_ref()
                                .and_then(|k| plugin.params.get(k))
                                .cloned()
                        })
                        .or_else(|| {
                            schema
                                .env
                                .iter()
                                .find_map(|k| plugin.params.get(k).cloned())
                        })
                        .or_else(|| plugin.params.get(key).cloned())
                        .or_else(|| {
                            schema.default.as_ref().map(|v| {
                                v.as_str()
                                    .map(str::to_owned)
                                    .unwrap_or_else(|| v.to_string())
                            })
                        })
                        .unwrap_or_default();
                    fields.push(Field {
                        plugin: plugin.name.clone(),
                        key: key.clone(),
                        schema: schema.clone(),
                        value: if schema.secret { String::new() } else { value },
                        locked: locked(key, schema, &plugin.manifest),
                    });
                }
            }
        }
        fields
    }

    pub fn apply(
        &mut self,
        router: &Arc<Mutex<McpRouter>>,
        field: &Field,
        value: String,
        shared: bool,
    ) -> Result<(), String> {
        let current = self
            .fields()
            .into_iter()
            .find(|f| f.plugin == field.plugin && f.key == field.key)
            .ok_or("Field is no longer available")?;
        if current.locked {
            return Err("Filesystem roots await sandbox support".into());
        }
        validate(&current.schema, &value)?;
        if shared && current.schema.secret {
            return Err("Secret values are per-plugin and session-only".into());
        }
        let mut next = self.settings.clone();
        if shared {
            next.shared.insert(field.key.clone(), value);
        } else {
            next.plugins
                .entry(field.plugin.clone())
                .or_default()
                .insert(field.key.clone(), value);
        }
        let mut router = router
            .try_lock()
            .map_err(|_| "Server is busy; retry after the active operation completes")?;
        let affected: Vec<_> = self
            .plugins
            .iter()
            .filter(|p| {
                p.name == field.plugin
                    || (shared
                        && p.manifest
                            .configuration
                            .as_ref()
                            .is_some_and(|c| c.fields.contains_key(&field.key)))
            })
            .collect();
        let mut replacements = Vec::new();
        for plugin in affected {
            if shared
                && plugin
                    .manifest
                    .configuration
                    .as_ref()
                    .is_some_and(|c| c.fields.get(&field.key).is_some_and(|f| f.secret))
            {
                return Err("Secret values cannot be shared defaults".into());
            }
            replacements.push((plugin.name.clone(), plugin.load(&next)?));
        }
        let mut persisted = next.clone();
        for plugin in &self.plugins {
            if let Some(config) = &plugin.manifest.configuration {
                for (key, schema) in &config.fields {
                    if schema.secret {
                        if let Some(values) = persisted.plugins.get_mut(&plugin.name) {
                            values.remove(key);
                        }
                        persisted.shared.remove(key);
                    }
                }
            }
        }
        let bytes =
            serde_json::to_vec_pretty(&persisted).map_err(|_| "Cannot serialize configuration")?;
        let lock_path = self.path.with_extension("lock");
        let lock_file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
            .map_err(|_| "Another configuration save is active; retry after it finishes")?;
        let _save_lock = SaveLock(lock_path);
        drop(lock_file);
        let disk = match std::fs::read(&self.path) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return Err("Cannot read saved configuration".into()),
        };
        if disk != self.original {
            return Err("Configuration changed outside this server; restart before editing".into());
        }
        let stage = self
            .path
            .with_extension(format!("{}.tmp", std::process::id()));
        let mut created = false;
        let result = (|| -> std::io::Result<()> {
            use std::io::Write;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&stage)?;
            created = true;
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&stage, &self.path)
        })();
        if result.is_err() {
            if created {
                let _ = std::fs::remove_file(stage);
            }
            return Err("Cannot save configuration; previous settings retained".into());
        }
        for (name, instance) in replacements {
            router.register(name, instance);
        }
        self.settings = next;
        self.original = Some(bytes);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_defaults_and_plugin_overrides_follow_native_and_wasm_mappings() {
        let manifest: PluginManifest = serde_json::from_value(serde_json::json!({"configuration":{"schema_version":1,"fields":{
            "timezone":{"type":"string","description":"Timezone","required":false,"secret":false,"env":["TIMEZONE","DEFAULT_TIMEZONE"],"config_key":"default_timezone"}
        }}})).unwrap();
        let mut plugin = Plugin {
            name: "time".into(),
            path: PathBuf::new(),
            kind: ExecutionKind::Native,
            manifest,
            params: Default::default(),
        };
        let mut settings = Settings::default();
        settings.shared.insert("timezone".into(), "UTC".into());
        assert_eq!(plugin.parameters(&settings).unwrap()["TIMEZONE"], "UTC");
        settings
            .plugins
            .entry("time".into())
            .or_default()
            .insert("timezone".into(), "Asia/Kolkata".into());
        assert_eq!(
            plugin.parameters(&settings).unwrap()["DEFAULT_TIMEZONE"],
            "Asia/Kolkata"
        );
        plugin.kind = ExecutionKind::Wasm;
        let params = plugin.parameters(&settings).unwrap();
        assert_eq!(params["default_timezone"], "Asia/Kolkata");
        assert!(!params.contains_key("TIMEZONE"));
    }
    #[test]
    fn typed_values_reject_invalid_numbers_booleans_and_terminal_controls() {
        let mut field: ConfigurationField = serde_json::from_value(serde_json::json!({"type":"integer", "description":"Count", "required":true, "secret":false, "env":["COUNT"]})).unwrap();
        assert!(validate(&field, "42").is_ok());
        assert!(validate(&field, "2.5").is_err());
        field.kind = "number".into();
        assert!(validate(&field, "NaN").is_err());
        field.kind = "boolean".into();
        assert!(validate(&field, "true").is_ok());
        assert!(validate(&field, "yes").is_err());
        field.kind = "string".into();
        assert!(validate(&field, "").is_err());
        assert!(validate(&field, "\u{1b}[2J").is_err());
    }
}
