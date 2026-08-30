// crates/rune-kit-core/src/package.rs
use crate::manifest::{InstalledPlugin, Lockfile};
use crate::runtime::WasmPluginInstance;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum PackageError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Network request failed: {0}")]
    Network(#[from] reqwest::Error),
    #[error("Plugin '{0}' not found in registry")]
    NotFound(String),
    #[error("Invalid package specification: {0}")]
    InvalidSpec(String),
}

pub struct PackageManager {
    base_dir: PathBuf,
    lockfile_path: PathBuf,
}

impl PackageManager {
    pub fn new() -> Self {
        let base_dir = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("rune-kit");

        let _ = std::fs::create_dir_all(base_dir.join("plugins"));
        let lockfile_path = base_dir.join("installed.json");

        Self {
            base_dir,
            lockfile_path,
        }
    }

    pub fn load_lockfile(&self) -> Lockfile {
        if let Ok(data) = std::fs::read_to_string(&self.lockfile_path) {
            serde_json::from_str(&data).unwrap_or_default()
        } else {
            Lockfile::default()
        }
    }

    pub fn save_lockfile(&self, lockfile: &Lockfile) -> Result<(), std::io::Error> {
        let content = serde_json::to_string_pretty(lockfile)?;
        std::fs::write(&self.lockfile_path, content)
    }

    pub fn get_plugin_path(&self, name: &str) -> Option<PathBuf> {
        let lockfile = self.load_lockfile();
        lockfile
            .plugins
            .get(name)
            .map(|p| self.base_dir.join(&p.binary_path))
    }

    pub async fn install(
        &self,
        target: &str,
        version: Option<String>,
    ) -> Result<InstalledPlugin, PackageError> {
        let (default_name, bytes, source) =
            if target.ends_with(".wasm") && Path::new(target).exists() {
                let path = Path::new(target);
                let stem = path.file_stem().unwrap().to_str().unwrap().to_string();
                let bytes = std::fs::read(path)?;
                (stem, bytes, target.to_string())
            } else if target.starts_with("http://") || target.starts_with("https://") {
                let res = reqwest::get(target).await?.bytes().await?;
                let name = target
                    .split('/')
                    .last()
                    .unwrap_or("plugin")
                    .replace(".wasm", "");
                (name, res.to_vec(), target.to_string())
            } else {
                self.fetch_from_registry(target, version.clone()).await?
            };

        // Probe the WASM binary for embedded compile-time metadata (name, version, description)
        let (name, ver, desc) =
            match WasmPluginInstance::load_from_bytes(&default_name, bytes.clone(), HashMap::new())
            {
                Ok(mut instance) => match instance.get_info() {
                    Ok(info) => {
                        let final_ver = version.unwrap_or(info.version);
                        let final_name = if default_name.is_empty() {
                            info.name
                        } else {
                            default_name
                        };
                        (final_name, final_ver, info.description)
                    }
                    Err(err) => {
                        eprintln!(
                            "[warn] Failed to call mcp_info on '{}': {}",
                            default_name, err
                        );
                        (
                            default_name,
                            version.unwrap_or_else(|| "0.1.0".to_string()),
                            None,
                        )
                    }
                },
                Err(err) => {
                    eprintln!(
                        "[warn] Extism failed to load '{}' during probe: {}",
                        default_name, err
                    );
                    (
                        default_name,
                        version.unwrap_or_else(|| "0.1.0".to_string()),
                        None,
                    )
                }
            };

        let hash: String = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect();

        let file_name = format!("{}-{}.wasm", name, ver);
        let dest = self.base_dir.join("plugins").join(&file_name);

        std::fs::write(&dest, bytes)?;

        let installed = InstalledPlugin {
            name: name.clone(),
            description: desc,
            version: ver,
            binary_path: format!("plugins/{}", file_name),
            sha256: hash,
            source,
            default_params: HashMap::new(),
        };

        let mut lockfile = self.load_lockfile();
        lockfile.plugins.insert(name, installed.clone());
        self.save_lockfile(&lockfile)?;

        Ok(installed)
    }

    pub fn uninstall(&self, name: &str) -> Result<bool, PackageError> {
        let mut lockfile = self.load_lockfile();
        if let Some(plugin) = lockfile.plugins.remove(name) {
            let binary = self.base_dir.join(&plugin.binary_path);
            if binary.exists() {
                let _ = std::fs::remove_file(binary);
            }
            self.save_lockfile(&lockfile)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn fetch_from_registry(
        &self,
        name: &str,
        version: Option<String>,
    ) -> Result<(String, Vec<u8>, String), PackageError> {
        let registry_url =
            "https://raw.githubusercontent.com/shba007/rune-tools/main/registry/index.json";
        let client = reqwest::Client::new();
        let index: serde_json::Value = client.get(registry_url).send().await?.json().await?;

        let pkg = index
            .get(name)
            .ok_or_else(|| PackageError::NotFound(name.to_string()))?;

        let ver = version.unwrap_or_else(|| pkg["latest"].as_str().unwrap_or("0.1.0").to_string());
        let download_url = pkg["versions"][&ver]["url"].as_str().ok_or_else(|| {
            PackageError::InvalidSpec("Missing download URL in registry index".into())
        })?;

        let bytes = client
            .get(download_url)
            .send()
            .await?
            .bytes()
            .await?
            .to_vec();

        Ok((name.to_string(), bytes, "registry".to_string()))
    }
}
