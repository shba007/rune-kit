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
    pub workspace: Option<PathBuf>,
}

impl Plugin {
    pub(crate) fn load_embedded(
        &self,
        settings: &Settings,
        staged: bool,
    ) -> Result<PluginInstance, String> {
        let mut plugin = self.clone();
        let mut settings = settings.clone();
        let mut root = settings
            .plugins
            .get_mut(&self.name)
            .and_then(|p| p.remove("allowed_dir"));
        if let Some(config) = &self.manifest.configuration {
            for (key, field) in &config.fields {
                if (field.config_key.as_deref() == Some("allowed_dir") || key == "allowed_dir")
                    && let Some(value) = settings
                        .plugins
                        .get_mut(&self.name)
                        .and_then(|p| p.remove(key))
                {
                    root = Some(value);
                }
            }
        }
        let staged_root = root.clone().or_else(|| {
            ["allowed_dir", "ALLOWED_DIR"]
                .iter()
                .find_map(|key| self.params.get(*key).cloned())
        });
        if let Some(root) = root {
            if self.workspace.is_some() {
                return Err(
                    "Embedded folder changes are unavailable in workspace-only mode".into(),
                );
            }
            plugin.params.insert("allowed_dir".into(), root.clone());
            plugin.params.insert("ALLOWED_DIR".into(), root.clone());
            if let Some(config) = &self.manifest.configuration {
                for (key, field) in &config.fields {
                    if key == "allowed_dir" || field.config_key.as_deref() == Some("allowed_dir") {
                        if field.secret {
                            return Err("Filesystem roots cannot be secret fields".into());
                        }
                        settings.shared.remove(key);
                        for env in &field.env {
                            plugin.params.insert(env.clone(), root.clone());
                        }
                    }
                }
            }
        }
        if staged && self.workspace.is_none() {
            let root = staged_root.ok_or("Staged mode needs a configured allowed_dir folder")?;
            return plugin.load_staged(&settings, &root);
        }
        plugin.load(&settings)
    }

    /// Per-plugin staged mode for embedded hosts. The real folder is never mounted
    /// into the guest; reads and writes go through the host file-access broker, so
    /// originals change only when the host applies a reviewed change.
    fn load_staged(&self, settings: &Settings, root: &str) -> Result<PluginInstance, String> {
        if self.kind != ExecutionKind::Wasm {
            return Err("Only WASM plugins can run in staged mode".into());
        }
        let root = validate_embedded_folder(root)?;
        let mut params = self.parameters(settings)?;
        params.insert("rune_staged".into(), "true".into());
        params.insert("rune_staged_root".into(), root.clone());
        let bytes = std::fs::read(&self.path).map_err(|_| "Cannot read plugin payload")?;
        let storage =
            rune_kit_core::paths::sandbox_root().map_err(|_| "Cannot resolve sandbox storage")?;
        let mut sandbox = rune_kit_core::sandbox::Sandbox::open(storage)
            .map_err(|_| "Cannot open sandbox storage")?;
        sandbox
            .approve(&self.name, std::path::Path::new(&root))
            .map_err(|_| "Cannot approve the folder for staged changes")?;
        WasmPluginInstance::load_with_file_access(
            &self.name,
            bytes,
            params,
            self.manifest.clone(),
            &sandbox,
        )
        .map(|p| PluginInstance::Wasm(Box::new(p)))
        .map_err(|_| "Cannot load staged WASM plugin; native helpers are unavailable".into())
    }

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
                    .or_else(|| {
                        if field.secret {
                            None
                        } else {
                            settings.shared.get(key)
                        }
                    })
                    .or_else(|| {
                        if field.secret {
                            self.params.get(key)
                        } else {
                            None
                        }
                    })
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
        if self.workspace.is_some()
            && self.kind == ExecutionKind::Native
            && !rune_kit_core::native_isolation::VALIDATED_ON_ALL_PLATFORMS
        {
            return Err("Native plugins are blocked in workspace-only mode".into());
        }
        match self.kind {
            ExecutionKind::Wasm => {
                let bytes = std::fs::read(&self.path).map_err(|_| "Cannot read plugin payload")?;
                if let Some(workspace) = &self.workspace {
                    let root=rune_kit_core::paths::sandbox_root().map_err(|_|"Cannot resolve sandbox storage")?;
                    let sandbox=rune_kit_core::sandbox::Sandbox::open(root).map_err(|_|"Cannot open sandbox storage")?;
                    if sandbox.workspace(&self.name).map_err(|_|"Cannot resolve plugin workspace")?!=*workspace {return Err("Plugin workspace does not match sandbox storage".into());}
                    return WasmPluginInstance::load_with_file_access(&self.name,bytes,params,self.manifest.clone(),&sandbox)
                        .map(|p|PluginInstance::Wasm(Box::new(p)))
                        .map_err(|_|"Cannot load workspace-only WASM plugin; native helpers and external paths are unavailable".into());
                }
                WasmPluginInstance::load_from_bytes(
                    &self.name,
                    bytes,
                    params,
                    self.manifest.clone(),
                )
                .map(|p| PluginInstance::Wasm(Box::new(p)))
            }
            ExecutionKind::Native => {
                if let Some(workspace)=&self.workspace { return NativeSidecar::new_in_workspace(&self.name,&self.path,params,self.manifest.clone(),workspace).map(|p|PluginInstance::Native(Box::new(p))).map_err(|_|"Cannot launch isolated native plugin; no unconfined fallback".into()); }
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
    embedded: bool,
    staged: std::collections::BTreeSet<String>,
}

pub(crate) struct SaveLock(PathBuf);
impl SaveLock {
    pub(crate) fn acquire(path: PathBuf) -> std::io::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        drop(file);
        Ok(Self(path))
    }
}
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
    pub(crate) fn enable_embedding(&mut self) {
        self.embedded = true;
    }
    pub(crate) fn set_embedded_root(
        &mut self,
        router: &Arc<Mutex<McpRouter>>,
        name: &str,
        key: &str,
        value: &str,
    ) -> Result<(), String> {
        let plugin = self
            .plugins
            .iter()
            .find(|p| p.name == name)
            .ok_or_else(|| format!("Unknown plugin '{name}'"))?;
        let field = plugin
            .manifest
            .configuration
            .as_ref()
            .and_then(|c| c.fields.get(key));
        if key != "allowed_dir"
            && !field.is_some_and(|f| f.config_key.as_deref() == Some("allowed_dir"))
        {
            return Err(
                "Embedded parameter changes are restricted to allowed_dir filesystem roots".into(),
            );
        }
        if field.is_some_and(|f| f.secret) {
            return Err("Filesystem roots cannot be secret fields".into());
        }
        if plugin.workspace.is_some() {
            return Err("Embedded folder changes are unavailable in workspace-only mode".into());
        }
        let value = validate_embedded_folder(value)?;
        let mut router = router
            .try_lock()
            .map_err(|_| "Server is busy; retry after the active operation completes")?;
        let _guard = SaveLock::acquire(self.path.with_extension("lock"))
            .map_err(|_| "Another configuration save is active; retry after it finishes")?;
        let disk = match std::fs::read(&self.path) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return Err("Cannot read saved configuration".into()),
        };
        if disk != self.original {
            return Err("Configuration changed outside this server; restart before editing".into());
        }
        let mut next = self.settings.clone();
        let values = next.plugins.entry(name.to_owned()).or_default();
        // Store the manifest field key, as setup does, and remove older root aliases.
        values.remove("allowed_dir");
        if let Some(config) = &plugin.manifest.configuration {
            for (key, field) in &config.fields {
                if field.config_key.as_deref() == Some("allowed_dir") {
                    values.remove(key);
                }
            }
        }
        values.insert(key.to_owned(), value);
        let replacement = plugin.load_embedded(&next, self.staged.contains(name))?;
        let mut persisted = next.clone();
        for plugin in &self.plugins {
            if let Some(config) = &plugin.manifest.configuration {
                for (key, field) in &config.fields {
                    if field.secret {
                        persisted.shared.remove(key);
                        if let Some(values) = persisted.plugins.get_mut(&plugin.name) {
                            values.remove(key);
                        }
                    }
                }
            }
        }
        let bytes =
            serde_json::to_vec_pretty(&persisted).map_err(|_| "Cannot serialize configuration")?;
        crate::setup::atomic(&self.path, &bytes)
            .map_err(|_| "Cannot save configuration; previous settings retained")?;
        router.register(name.to_owned(), replacement);
        self.settings = next;
        self.original = Some(bytes);
        Ok(())
    }

    /// Switch one plugin between direct and staged file access. Nothing is saved to
    /// disk; the setting lasts for this server's lifetime.
    pub(crate) fn set_embedded_staged(
        &mut self,
        router: &Arc<Mutex<McpRouter>>,
        name: &str,
        staged: bool,
    ) -> Result<(), String> {
        let plugin = self
            .plugins
            .iter()
            .find(|p| p.name == name)
            .ok_or_else(|| format!("Unknown plugin '{name}'"))?;
        if plugin.workspace.is_some() {
            return Err("Plugin is already staged by workspace-only mode".into());
        }
        let mut router = router
            .try_lock()
            .map_err(|_| "Server is busy; retry after the active operation completes")?;
        let replacement = plugin.load_embedded(&self.settings, staged)?;
        router.register(name.to_owned(), replacement);
        if staged {
            self.staged.insert(name.to_owned());
        } else {
            self.staged.remove(name);
        }
        Ok(())
    }

    pub(crate) fn is_embedded_staged(&self, name: &str) -> bool {
        self.staged.contains(name)
    }

    pub(crate) fn embedded_params(&self, name: &str) -> BTreeMap<String, String> {
        let Some(plugin) = self.plugins.iter().find(|p| p.name == name) else {
            return BTreeMap::new();
        };
        let mut values: BTreeMap<_, _> = self
            .fields()
            .into_iter()
            .filter(|f| f.plugin == name && !f.schema.secret)
            .map(|f| (f.key, f.value))
            .collect();
        let root = self
            .settings
            .plugins
            .get(name)
            .and_then(|p| p.get("allowed_dir"))
            .or_else(|| {
                plugin.manifest.configuration.as_ref().and_then(|c| {
                    c.fields.iter().find_map(|(key, field)| {
                        (field.config_key.as_deref() == Some("allowed_dir") && !field.secret)
                            .then(|| self.settings.plugins.get(name).and_then(|p| p.get(key)))
                            .flatten()
                    })
                })
            })
            .or_else(|| plugin.params.get("allowed_dir"))
            .or_else(|| plugin.params.get("ALLOWED_DIR"));
        if let Some(root) = root {
            let secret_root = plugin.manifest.configuration.as_ref().is_some_and(|c| {
                c.fields.iter().any(|(key, field)| {
                    field.secret
                        && (key == "allowed_dir"
                            || field.config_key.as_deref() == Some("allowed_dir"))
                })
            });
            if !secret_root {
                values.insert("allowed_dir".into(), root.clone());
                if let Some(config) = &plugin.manifest.configuration {
                    for (key, field) in &config.fields {
                        if !field.secret
                            && (key == "allowed_dir"
                                || field.config_key.as_deref() == Some("allowed_dir"))
                        {
                            values.insert(key.clone(), root.clone());
                        }
                    }
                }
            }
        }
        values
    }

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
            embedded: false,
            staged: Default::default(),
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
            let replacement = if self.embedded {
                plugin.load_embedded(&next, self.staged.contains(&plugin.name))?
            } else {
                plugin.load(&next)?
            };
            replacements.push((plugin.name.clone(), replacement));
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

// Only the embedding API uses this validator; setup's field selection is unchanged.
fn validate_embedded_folder(value: &str) -> Result<String, String> {
    let path = std::path::Path::new(value);
    if !path.is_absolute() || !path.is_dir() {
        return Err("Filesystem root must be an existing absolute directory".into());
    }
    let path = std::fs::canonicalize(path).map_err(|_| "Cannot canonicalise filesystem root")?;
    let path = path.to_str().ok_or("Filesystem root must be valid UTF-8")?;
    #[cfg(windows)]
    let path = if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else {
        path.strip_prefix(r"\\?\").unwrap_or(path).to_owned()
    };
    Ok(path.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn workspace_mode_cannot_reload_a_native_plugin() {
        let root = tempfile::tempdir().unwrap();
        let plugin = Plugin {
            name: "native".into(),
            path: PathBuf::new(),
            kind: ExecutionKind::Native,
            manifest: PluginManifest::default(),
            params: Default::default(),
            workspace: Some(root.path().to_owned()),
        };
        assert!(
            plugin
                .load(&Settings::default())
                .err()
                .unwrap()
                .contains("Native plugins are blocked")
        );
    }
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
            workspace: None,
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
