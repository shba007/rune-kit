use crate::manifest::{
    ExecutionKind, InstalledPlugin, InstalledSkill, Lockfile, PluginManifest, PluginUpdateStatus,
    RegistryPluginSummary, RegistrySkillSummary, SkillFile, SkillLockfile,
};
use crate::provision::BinaryProvisioner;
use crate::runtime::{NativeSidecar, WasmPluginInstance};
use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use tar::Archive;
use thiserror::Error;

struct PackageLock(PathBuf);
impl PackageLock {
    fn acquire(root: &Path) -> Result<Self, PackageError> {
        std::fs::create_dir_all(root)?;
        let path = root.join("packages.lock");
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|_| PackageError::InvalidSpec("Another package operation is active".into()))?;
        Ok(Self(path))
    }
}
impl Drop for PackageLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn read_install_records<T: serde::de::DeserializeOwned + Default>(
    path: &Path,
) -> Result<T, PackageError> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| {
            PackageError::InvalidSpec(
                "Installed package records are malformed; existing files were preserved".into(),
            )
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(error) => Err(error.into()),
    }
}

fn record_bytes(path: &Path) -> Result<Option<Vec<u8>>, PackageError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn save_records(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let stage = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&stage)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&stage, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(stage);
    }
    result
}

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
    #[error("Runtime initialization probe failed: {0}")]
    Runtime(#[from] crate::runtime::RuntimeError),
    #[error("Dependency provisioning failed: {0}")]
    Provision(#[from] crate::provision::ProvisionError),
    #[error("Self-update error: {0}")]
    SelfUpdate(String),
}

pub struct FetchedBundle {
    pub name: String,
    pub version: Option<String>,
    pub wasm_bytes: Option<Vec<u8>>,
    pub native_bytes: Option<Vec<u8>>,
    pub manifest_toml: Option<String>,
    pub source: String,
}

struct CliAsset {
    tag: String,
    url: String,
    digest: Option<String>,
    sidecar: Option<String>,
}

fn verify_cli_checksum(bytes: &[u8], expected: &str) -> Result<(), PackageError> {
    if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(PackageError::SelfUpdate(
            "Invalid release SHA-256 checksum".into(),
        ));
    }
    let actual: String = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(PackageError::SelfUpdate(
            "Release checksum mismatch; existing installation was not changed".into(),
        ));
    }
    Ok(())
}
fn activate_cli_update(executable: &Path, binary: &[u8]) -> Result<(), PackageError> {
    let parent = executable
        .parent()
        .ok_or_else(|| PackageError::SelfUpdate("Cannot resolve executable directory".into()))?;
    let stage = parent.join(format!(
        ".rune-kit-update-{}-{}.tmp",
        std::process::id(),
        chrono_timestamp()
    ));
    let result = (|| -> Result<(), PackageError> {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&stage)?;
        file.write_all(binary)?;
        file.sync_all()?;
        drop(file);
        set_executable_permissions(&stage)?;
        #[cfg(windows)]
        {
            let backup = parent.join(format!(
                ".rune-kit-old-{}-{}.tmp",
                std::process::id(),
                chrono_timestamp()
            ));
            std::fs::rename(executable, &backup)?;
            if let Err(e) = std::fs::rename(&stage, executable) {
                let _ = std::fs::rename(&backup, executable);
                return Err(e.into());
            }
            let _ = std::fs::remove_file(backup);
        }
        #[cfg(not(windows))]
        {
            std::fs::rename(&stage, executable)?;
        }
        Ok(())
    })();
    let _ = std::fs::remove_file(&stage);
    result
}

pub struct PackageManager {
    base_dir: PathBuf,
    lockfile_path: PathBuf,
    provisioner: BinaryProvisioner,
}

impl Default for PackageManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PackageManager {
    /// Read configuration for an exact published plugin version without installing it.
    pub async fn fetch_version_metadata(
        &self,
        name: &str,
        version: &str,
    ) -> Result<crate::registry::VersionMetadata, PackageError> {
        let index: serde_json::Value = reqwest::Client::new()
            .get("https://raw.githubusercontent.com/shba007/rune-tools/refs/heads/main/registry/index.json")
            .send().await?.error_for_status()?.json().await?;
        crate::registry::VersionMetadata::from_value(&index[name]["versions"][version])
    }

    pub fn new() -> Self {
        let base_dir = crate::paths::managed_root().expect("Cannot initialize Kit storage");

        let _ = std::fs::create_dir_all(base_dir.join("plugins"));
        let lockfile_path = base_dir.join("installed.json");

        Self {
            base_dir,
            lockfile_path,
            provisioner: BinaryProvisioner::new(),
        }
    }

    pub fn base_dir(&self) -> &Path {
        &self.base_dir
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
        save_records(&self.lockfile_path, content.as_bytes())
    }

    pub fn get_plugin_path(&self, name: &str) -> Option<PathBuf> {
        let lockfile = self.load_lockfile();
        lockfile
            .plugins
            .get(name)
            .map(|p| self.base_dir.join(&p.binary_path))
    }

    pub async fn fetch_registry(&self) -> Result<Vec<RegistryPluginSummary>, PackageError> {
        let registry_url = "https://raw.githubusercontent.com/shba007/rune-tools/refs/heads/main/registry/index.json";
        let client = reqwest::Client::builder().user_agent("rune-kit").build()?;
        let index: serde_json::Value = client
            .get(registry_url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let mut results = Vec::new();
        if let Some(map) = index.as_object() {
            for (name, pkg) in map {
                let latest = pkg
                    .get("latest")
                    .and_then(|v| v.as_str())
                    .unwrap_or("0.1.0")
                    .to_string();

                let description = pkg
                    .get("description")
                    .and_then(|v| v.as_str())
                    .map(ToString::to_string);

                let mut has_wasm = false;
                let mut has_native = false;

                if let Some(ver_obj) = pkg.get("versions").and_then(|v| v.get(&latest)) {
                    has_wasm = ver_obj.get("url").and_then(|u| u.as_str()).is_some();
                    has_native = ver_obj.get("native").is_some();
                }

                results.push(RegistryPluginSummary {
                    name: name.clone(),
                    latest,
                    description,
                    has_wasm,
                    has_native,
                });
            }
        }
        results.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(results)
    }

    pub async fn check_plugin_updates(
        &self,
        names: Option<&[String]>,
    ) -> Result<Vec<PluginUpdateStatus>, PackageError> {
        let lockfile = self.load_lockfile();
        let registry = self.fetch_registry().await?;
        let registry_map: HashMap<String, RegistryPluginSummary> =
            registry.into_iter().map(|p| (p.name.clone(), p)).collect();

        let mut statuses = Vec::new();
        for (name, installed) in &lockfile.plugins {
            if let Some(filter) = names
                && !filter.iter().any(|n| n.eq_ignore_ascii_case(name))
            {
                continue;
            }

            if let Some(reg_entry) = registry_map.get(name) {
                let has_update = is_newer_version(&reg_entry.latest, &installed.version);
                statuses.push(PluginUpdateStatus {
                    name: name.clone(),
                    installed_version: installed.version.clone(),
                    latest_version: reg_entry.latest.clone(),
                    has_update,
                    has_wasm: reg_entry.has_wasm,
                    has_native: reg_entry.has_native,
                });
            }
        }

        statuses.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(statuses)
    }

    pub async fn fetch_latest_cli_release(
        &self,
        version: Option<&str>,
    ) -> Result<(String, String), PackageError> {
        let asset = self.cli_release_asset(version).await?;
        Ok((asset.tag, asset.url))
    }

    async fn cli_release_asset(
        &self,
        requested_version: Option<&str>,
    ) -> Result<CliAsset, PackageError> {
        let client = reqwest::Client::builder().user_agent("rune-kit").build()?;

        let url = match requested_version {
            Some(ver) => {
                let tag = if ver.starts_with('v') {
                    ver.to_string()
                } else {
                    format!("v{}", ver)
                };
                format!(
                    "https://api.github.com/repos/shba007/rune-kit/releases/tags/{}",
                    tag
                )
            }
            None => "https://api.github.com/repos/shba007/rune-kit/releases/latest".to_string(),
        };

        let resp = client.get(&url).send().await?;
        if !resp.status().is_success() {
            return Err(PackageError::SelfUpdate(format!(
                "GitHub release lookup failed with status: {}",
                resp.status()
            )));
        }

        let release: serde_json::Value = resp.json().await?;
        let tag_name = release
            .get("tag_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PackageError::SelfUpdate("Release missing tag_name".to_string()))?
            .to_string();

        let triple = current_target_triple();
        let assets = release
            .get("assets")
            .and_then(|a| a.as_array())
            .ok_or_else(|| PackageError::SelfUpdate("Release missing assets".to_string()))?;

        let asset = assets
            .iter()
            .find(|a| {
                let name = a.get("name").and_then(|n| n.as_str()).unwrap_or("");
                name.contains(triple) && (name.ends_with(".tar.gz") || name.ends_with(".zip"))
            })
            .ok_or_else(|| {
                PackageError::SelfUpdate(format!(
                    "No compatible binary found for target '{}' in release '{}'",
                    triple, tag_name
                ))
            })?;

        let download_url = asset
            .get("browser_download_url")
            .and_then(|u| u.as_str())
            .ok_or_else(|| PackageError::SelfUpdate("Asset missing download URL".to_string()))?
            .to_string();

        let digest = asset
            .get("digest")
            .and_then(|v| v.as_str())
            .and_then(|v| v.strip_prefix("sha256:"))
            .map(str::to_owned);
        let sidecar_name = format!("{}.sha256", asset["name"].as_str().unwrap_or(""));
        let sidecar = assets
            .iter()
            .find(|a| a["name"].as_str() == Some(&sidecar_name))
            .and_then(|a| a["browser_download_url"].as_str())
            .map(str::to_owned);
        Ok(CliAsset {
            tag: tag_name,
            url: download_url,
            digest,
            sidecar,
        })
    }

    pub async fn self_update(
        &self,
        requested_version: Option<&str>,
        force: bool,
    ) -> Result<String, PackageError> {
        let current_ver = env!("CARGO_PKG_VERSION");
        let asset = self.cli_release_asset(requested_version).await?;
        let tag = asset.tag;
        let download_url = asset.url;
        let target_ver = tag.strip_prefix('v').unwrap_or(&tag);

        if !force && !is_newer_version(target_ver, current_ver) && target_ver == current_ver {
            return Err(PackageError::SelfUpdate(format!(
                "Already on the latest version (v{}). Use --force to overwrite.",
                current_ver
            )));
        }

        let client = reqwest::Client::builder().user_agent("rune-kit").build()?;
        let expected = if let Some(digest) = asset.digest {
            digest
        } else if let Some(url) = asset.sidecar {
            client
                .get(url)
                .send()
                .await?
                .error_for_status()?
                .text()
                .await?
                .split_whitespace()
                .next()
                .ok_or_else(|| PackageError::SelfUpdate("Empty release checksum sidecar".into()))?
                .to_owned()
        } else {
            return Err(PackageError::SelfUpdate(
                "Release has no SHA-256 digest or sidecar; existing executable was not changed"
                    .into(),
            ));
        };
        let archive_bytes = client
            .get(&download_url)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?
            .to_vec();

        verify_cli_checksum(&archive_bytes, &expected)?;
        let binary_bytes = extract_single_binary(&archive_bytes, "rune-kit")?;
        // Update the invoked executable, including explicit custom locations.
        // The installer establishes ~/.rune; never replace Infer or a second Kit.
        activate_cli_update(&std::env::current_exe()?, &binary_bytes)?;

        Ok(tag)
    }

    pub async fn install(
        &self,
        target: &str,
        version: Option<String>,
        prefer_native: bool,
    ) -> Result<InstalledPlugin, PackageError> {
        self.install_internal(target, version, prefer_native, false, false)
            .await
    }

    pub async fn install_new(
        &self,
        target: &str,
        version: Option<String>,
        prefer_native: bool,
    ) -> Result<InstalledPlugin, PackageError> {
        self.install_internal(target, version, prefer_native, true, true)
            .await
    }

    pub async fn update_installed(
        &self,
        target: &str,
        version: Option<String>,
    ) -> Result<InstalledPlugin, PackageError> {
        self.install_internal(target, version, false, false, true)
            .await
    }

    async fn install_internal(
        &self,
        target: &str,
        version: Option<String>,
        prefer_native: bool,
        new_only: bool,
        absolute_only: bool,
    ) -> Result<InstalledPlugin, PackageError> {
        let _lock = PackageLock::acquire(&self.base_dir)?;
        let original_records = record_bytes(&self.lockfile_path)?;
        let existing: Lockfile = read_install_records(&self.lockfile_path)?;
        let target_path = Path::new(target);

        let bundle = if (!absolute_only || target_path.is_absolute()) && target_path.exists() {
            if target.ends_with(".wasm") {
                let stem = target_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .ok_or_else(|| {
                        PackageError::InvalidSpec(format!(
                            "Cannot derive a plugin name from path '{}'",
                            target
                        ))
                    })?
                    .replace('_', "-");
                let bytes = std::fs::read(target_path)?;
                let parent = target_path.parent().unwrap_or_else(|| Path::new("."));

                let manifest_toml = {
                    let m_path = parent.join("plugin.toml");
                    if m_path.exists() {
                        std::fs::read_to_string(m_path).ok()
                    } else {
                        None
                    }
                };

                let sidecar_candidates = [
                    parent.join(format!("rune-{}-native.exe", stem)),
                    parent.join(format!("rune-{}-native", stem)),
                    parent.join(format!("{}-native.exe", stem)),
                    parent.join(format!("{}-native", stem)),
                ];
                let native_bytes = sidecar_candidates
                    .iter()
                    .find(|p| p.exists())
                    .and_then(|p| std::fs::read(p).ok());

                FetchedBundle {
                    name: stem,
                    version,
                    wasm_bytes: Some(bytes),
                    native_bytes,
                    manifest_toml,
                    source: target.to_string(),
                }
            } else if is_native_archive_or_binary(target) {
                let stem = target_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("plugin")
                    .to_string();
                let bytes = std::fs::read(target_path)?;
                let parent = target_path.parent().unwrap_or_else(|| Path::new("."));

                let manifest_toml = {
                    let m_path = parent.join("plugin.toml");
                    if m_path.exists() {
                        std::fs::read_to_string(m_path).ok()
                    } else {
                        None
                    }
                };

                FetchedBundle {
                    name: stem,
                    version,
                    wasm_bytes: None,
                    native_bytes: Some(bytes),
                    manifest_toml,
                    source: target.to_string(),
                }
            } else {
                return Err(PackageError::InvalidSpec(format!(
                    "Unrecognized plugin file format: '{}'",
                    target
                )));
            }
        } else if target.starts_with("http://") || target.starts_with("https://") {
            let res = reqwest::get(target).await?.bytes().await?.to_vec();
            let name = target
                .split('/')
                .next_back()
                .unwrap_or("plugin")
                .replace(".wasm", "")
                .replace(".zip", "")
                .replace(".tar.gz", "")
                .replace(".exe", "");

            if target.ends_with(".wasm") {
                FetchedBundle {
                    name,
                    version,
                    wasm_bytes: Some(res),
                    native_bytes: None,
                    manifest_toml: None,
                    source: target.to_string(),
                }
            } else {
                FetchedBundle {
                    name,
                    version,
                    wasm_bytes: None,
                    native_bytes: Some(res),
                    manifest_toml: None,
                    source: target.to_string(),
                }
            }
        } else {
            self.fetch_from_registry(target, version.clone(), prefer_native)
                .await?
        };

        let plugins_dir = self.base_dir.join("plugins");
        sanitize_path_component(&bundle.name, "plugin name")?;
        if new_only && existing.plugins.contains_key(&bundle.name) {
            return Err(PackageError::InvalidSpec(
                "Plugin already installed; existing files were preserved".into(),
            ));
        }
        if absolute_only && !new_only && !existing.plugins.contains_key(&bundle.name) {
            return Err(PackageError::InvalidSpec(
                "Plugin is no longer installed".into(),
            ));
        }
        if new_only && let Some(bytes) = record_bytes(&self.base_dir.join("remotes.json"))? {
            let remotes: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| {
                PackageError::InvalidSpec(
                    "Remote server records are malformed; existing files were preserved".into(),
                )
            })?;
            let servers = remotes["servers"].as_object().ok_or_else(|| {
                PackageError::InvalidSpec(
                    "Remote server records are malformed; existing files were preserved".into(),
                )
            })?;
            if servers.contains_key(&bundle.name) {
                return Err(PackageError::InvalidSpec(
                    "A remote server already uses this plugin name".into(),
                ));
            }
        }
        std::fs::create_dir_all(&plugins_dir)?;
        if std::fs::symlink_metadata(&plugins_dir)?
            .file_type()
            .is_symlink()
        {
            return Err(PackageError::InvalidSpec(
                "Plugin storage is a symbolic link; existing files were preserved".into(),
            ));
        }

        let manifest = if let Some(ref toml_str) = bundle.manifest_toml {
            PluginManifest::from_toml(toml_str).unwrap_or_default()
        } else {
            PluginManifest::default()
        };

        for (dep_name, dep_spec) in &manifest.dependencies.binaries {
            if !dep_spec.optional {
                let _ = self
                    .provisioner
                    .provision_binary(dep_name, dep_spec)
                    .await?;
            }
        }

        let mut probe_params: HashMap<String, String> = crate::runtime::resolve_host_env_params();
        probe_params.entry("allowed_hosts".to_string()).or_default();

        let (mut final_name, mut final_ver, mut final_desc) = (
            bundle.name.clone(),
            bundle
                .version
                .clone()
                .unwrap_or_else(|| "0.1.0".to_string()),
            None,
        );
        sanitize_path_component(&final_ver, "plugin version")?;

        let mut wasm_rel_path = None;
        let mut sha256_hash = String::new();

        if let Some(ref wasm_bytes) = bundle.wasm_bytes {
            if let Ok(mut instance) = WasmPluginInstance::load_from_bytes(
                &bundle.name,
                wasm_bytes.clone(),
                probe_params.clone(),
                manifest.clone(),
            ) && let Ok(info) = instance.get_info()
            {
                final_name = if bundle.name.is_empty() {
                    info.name
                } else {
                    bundle.name.clone()
                };
                final_ver = bundle.version.clone().unwrap_or(info.version);
                final_desc = info.description;
            }

            sha256_hash = Sha256::digest(wasm_bytes)
                .iter()
                .map(|b| format!("{:02x}", b))
                .collect();

            let file_name = format!("{}-{}.wasm", final_name, final_ver);
            let dest_wasm = plugins_dir.join(&file_name);
            if std::fs::symlink_metadata(&dest_wasm)
                .is_ok_and(|metadata| metadata.file_type().is_symlink())
            {
                return Err(PackageError::InvalidSpec(
                    "Plugin destination is a symbolic link; existing files were preserved".into(),
                ));
            }
            if new_only {
                use std::io::Write;
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&dest_wasm)?
                    .write_all(wasm_bytes)?;
            } else {
                std::fs::write(&dest_wasm, wasm_bytes)?;
            }
            wasm_rel_path = Some(format!("plugins/{}", file_name));
        }

        let mut native_rel_path = None;
        if let Some(ref native_bytes) = bundle.native_bytes {
            let extract_res = (|| -> Result<String, PackageError> {
                let temp_dir = plugins_dir.join(format!(".tmp-native-{}", chrono_timestamp()));
                std::fs::create_dir_all(&temp_dir)?;

                let extracted_binary =
                    extract_archive_or_binary(native_bytes, &temp_dir, &final_name)?;
                set_executable_permissions(&extracted_binary)?;

                if final_desc.is_none()
                    && let Ok(mut sidecar) = NativeSidecar::new(
                        &final_name,
                        &extracted_binary,
                        probe_params.clone(),
                        Some(manifest.clone()),
                    )
                    && let Ok(info) = sidecar.get_info()
                {
                    final_ver = bundle.version.clone().unwrap_or(info.version);
                    final_desc = info.description;
                }

                let bin_file_name = extracted_binary
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(&final_name)
                    .to_string();

                let final_bin_path = plugins_dir.join(&bin_file_name);
                if new_only {
                    std::fs::hard_link(&extracted_binary, &final_bin_path)?;
                    std::fs::remove_file(&extracted_binary)?;
                    set_executable_permissions(&final_bin_path)?;
                    let _ = std::fs::remove_dir_all(&temp_dir);
                    return Ok(format!("plugins/{}", bin_file_name));
                }
                if final_bin_path.exists() {
                    let _ = std::fs::remove_file(&final_bin_path);
                }
                if std::fs::rename(&extracted_binary, &final_bin_path).is_err() {
                    std::fs::copy(&extracted_binary, &final_bin_path)?;
                    let _ = std::fs::remove_file(&extracted_binary);
                }
                set_executable_permissions(&final_bin_path)?;
                let _ = std::fs::remove_dir_all(&temp_dir);

                Ok(format!("plugins/{}", bin_file_name))
            })();

            match extract_res {
                Ok(rel_path) => {
                    native_rel_path = Some(rel_path);
                }
                Err(err) => {
                    if wasm_rel_path.is_some() && !prefer_native {
                        eprintln!(
                            "[warn] Optional native binary extraction failed ({}). Continuing with WASM build.",
                            err
                        );
                    } else {
                        return Err(err);
                    }
                }
            }
        }

        sanitize_path_component(&final_name, "plugin name")?;
        sanitize_path_component(&final_ver, "plugin version")?;

        let execution_kind = if prefer_native && native_rel_path.is_some() {
            ExecutionKind::Native
        } else if wasm_rel_path.is_some() {
            ExecutionKind::Wasm
        } else if native_rel_path.is_some() {
            ExecutionKind::Native
        } else {
            return Err(PackageError::InvalidSpec(
                "Neither WASM nor Native binary was provided".to_string(),
            ));
        };

        let primary_binary_path = match execution_kind {
            ExecutionKind::Wasm => wasm_rel_path
                .clone()
                .ok_or_else(|| PackageError::InvalidSpec("WASM binary path missing".to_string()))?,
            ExecutionKind::Native => native_rel_path.clone().ok_or_else(|| {
                PackageError::InvalidSpec("Native binary path missing".to_string())
            })?,
        };

        let installed = InstalledPlugin {
            name: final_name.clone(),
            description: final_desc,
            version: final_ver,
            binary_path: primary_binary_path,
            native_binary_path: native_rel_path,
            execution_kind,
            sha256: sha256_hash,
            source: bundle.source,
            default_params: HashMap::new(),
            manifest: Some(manifest),
        };

        if record_bytes(&self.lockfile_path)? != original_records {
            return Err(PackageError::InvalidSpec(
                "Package records changed during installation; existing records were preserved"
                    .into(),
            ));
        }
        let mut lockfile = existing;
        lockfile.plugins.insert(final_name, installed.clone());
        self.save_lockfile(&lockfile)?;

        Ok(installed)
    }

    pub fn uninstall(&self, name: &str) -> Result<bool, PackageError> {
        let _lock = PackageLock::acquire(&self.base_dir)?;
        let mut lockfile: Lockfile = read_install_records(&self.lockfile_path)?;
        if let Some(plugin) = lockfile.plugins.remove(name) {
            for path in std::iter::once(&plugin.binary_path).chain(plugin.native_binary_path.iter())
            {
                let path = Path::new(path);
                let mut components = path.components();
                if components.next() != Some(std::path::Component::Normal("plugins".as_ref()))
                    || components
                        .any(|component| !matches!(component, std::path::Component::Normal(_)))
                {
                    return Err(PackageError::InvalidSpec(
                        "Plugin path is outside managed storage".into(),
                    ));
                }
                let base = self.base_dir.join("plugins");
                if std::fs::symlink_metadata(&base)?.file_type().is_symlink() {
                    return Err(PackageError::InvalidSpec(
                        "Plugin storage is a symbolic link".into(),
                    ));
                }
                let full = self.base_dir.join(path);
                let parent = full
                    .parent()
                    .ok_or_else(|| PackageError::InvalidSpec("Invalid plugin path".into()))?;
                if !std::fs::canonicalize(parent)?.starts_with(std::fs::canonicalize(base)?) {
                    return Err(PackageError::InvalidSpec(
                        "Plugin path is outside managed storage".into(),
                    ));
                }
            }
            let binary = self.base_dir.join(&plugin.binary_path);
            if binary.exists() {
                std::fs::remove_file(binary)?;
            }
            if let Some(ref native_rel) = plugin.native_binary_path {
                let native_bin = self.base_dir.join(native_rel);
                if native_bin.exists() {
                    std::fs::remove_file(native_bin)?;
                }
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
        prefer_native: bool,
    ) -> Result<FetchedBundle, PackageError> {
        let registry_url = "https://raw.githubusercontent.com/shba007/rune-tools/refs/heads/main/registry/index.json";
        let client = reqwest::Client::new();
        let index: serde_json::Value = client
            .get(registry_url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let pkg = index
            .get(name)
            .ok_or_else(|| PackageError::NotFound(name.to_string()))?;

        let ver = version.unwrap_or_else(|| pkg["latest"].as_str().unwrap_or("0.1.0").to_string());
        let ver_obj = &pkg["versions"][&ver];
        let metadata = crate::registry::VersionMetadata::from_value(ver_obj)?;
        let triple = current_target_triple();

        let native_url = ver_obj
            .get("native")
            .and_then(|n| {
                n.get(triple)
                    .or_else(|| n.get(std::env::consts::OS))
                    .or_else(|| {
                        n.get(format!(
                            "{}-{}",
                            std::env::consts::OS,
                            std::env::consts::ARCH
                        ))
                    })
            })
            .and_then(|v| v.get("url").and_then(|u| u.as_str()).or_else(|| v.as_str()));

        let wasm_url = ver_obj.get("url").and_then(|u| u.as_str());

        if native_url.is_none() && wasm_url.is_none() {
            return Err(PackageError::NotFound(format!(
                "No artifacts found in registry for '{}' (v{})",
                name, ver
            )));
        }

        // Validate all selected checksum entries before downloading or probing code.
        for url in [wasm_url, native_url].into_iter().flatten() {
            metadata.checksum(url)?;
        }
        let wasm_bytes = if let Some(url) = wasm_url {
            let bytes = download_plugin(&client, url, metadata.checksum(url)?).await?;
            Some(bytes)
        } else {
            None
        };

        let native_bytes = if let Some(url) = native_url {
            let bytes = download_plugin(&client, url, metadata.checksum(url)?).await?;
            Some(bytes)
        } else {
            if prefer_native {
                eprintln!(
                    "[warn] No native build available for host triple '{}' on plugin '{}'. Falling back to WASM.",
                    triple, name
                );
            }
            None
        };

        Ok(FetchedBundle {
            name: name.to_string(),
            version: Some(ver),
            wasm_bytes,
            native_bytes,
            manifest_toml: Some(
                toml::to_string(&PluginManifest {
                    configuration: Some(metadata.configuration),
                    ..Default::default()
                })
                .map_err(|e| PackageError::InvalidSpec(e.to_string()))?,
            ),
            source: "registry".to_string(),
        })
    }
}

async fn download_plugin(
    client: &reqwest::Client,
    url: &str,
    checksum: &crate::registry::ArtifactChecksum,
) -> Result<Vec<u8>, PackageError> {
    let mut response = client.get(url).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|size| size != checksum.size)
    {
        return Err(PackageError::InvalidSpec(
            "Plugin artifact size mismatch; installation was not changed".into(),
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if (bytes.len() as u64).saturating_add(chunk.len() as u64) > checksum.size {
            return Err(PackageError::InvalidSpec(
                "Plugin artifact exceeds published size; installation was not changed".into(),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    checksum.verify(&bytes)?;
    Ok(bytes)
}

pub struct SkillManager {
    base_dir: PathBuf,
    lockfile_path: PathBuf,
}

impl Default for SkillManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SkillManager {
    pub fn new() -> Self {
        let base_dir = crate::paths::managed_root().expect("Cannot initialize Kit storage");

        let _ = std::fs::create_dir_all(base_dir.join("skills"));
        let lockfile_path = base_dir.join("installed_skills.json");

        Self {
            base_dir,
            lockfile_path,
        }
    }

    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    pub fn load_skills_lockfile(&self) -> SkillLockfile {
        if let Ok(data) = std::fs::read_to_string(&self.lockfile_path) {
            serde_json::from_str(&data).unwrap_or_default()
        } else {
            SkillLockfile::default()
        }
    }

    pub fn save_skills_lockfile(&self, lockfile: &SkillLockfile) -> Result<(), std::io::Error> {
        let content = serde_json::to_string_pretty(lockfile)?;
        save_records(&self.lockfile_path, content.as_bytes())
    }

    pub async fn fetch_skills(&self) -> Result<Vec<RegistrySkillSummary>, PackageError> {
        let skills_url = "https://raw.githubusercontent.com/shba007/rune-tools/refs/heads/main/registry/index.json";
        let client = reqwest::Client::new();
        let index: serde_json::Value = client.get(skills_url).send().await?.json().await?;

        let mut results = Vec::new();
        if let Some(map) = index.as_object() {
            for (name, item) in map {
                // Strictly filter for skill entries; plugin packages in rune-tools do not qualify as skills
                let is_skill = item.get("type").and_then(|v| v.as_str()) == Some("skill")
                    || item.get("kind").and_then(|v| v.as_str()) == Some("skill");

                if !is_skill {
                    continue;
                }

                let latest = item
                    .get("latest")
                    .and_then(|v| v.as_str())
                    .unwrap_or("0.1.0")
                    .to_string();
                let description = item
                    .get("description")
                    .and_then(|v| v.as_str())
                    .map(ToString::to_string);
                let author = item
                    .get("author")
                    .and_then(|v| v.as_str())
                    .map(ToString::to_string);
                let tags = item
                    .get("tags")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|t| t.as_str().map(ToString::to_string))
                            .collect()
                    })
                    .unwrap_or_default();

                results.push(RegistrySkillSummary {
                    name: name.clone(),
                    latest,
                    description,
                    author,
                    tags,
                });
            }
        }
        results.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(results)
    }

    async fn fetch_skill_entry(&self, name: &str) -> Result<serde_json::Value, PackageError> {
        let skills_url = "https://raw.githubusercontent.com/shba007/rune-tools/refs/heads/main/registry/index.json";
        let client = reqwest::Client::new();
        let index: serde_json::Value = client.get(skills_url).send().await?.json().await?;
        let entry = index
            .get(name)
            .cloned()
            .ok_or_else(|| PackageError::NotFound(name.to_string()))?;

        let is_skill = entry.get("type").and_then(|v| v.as_str()) == Some("skill")
            || entry.get("kind").and_then(|v| v.as_str()) == Some("skill");
        if !is_skill {
            return Err(PackageError::NotFound(format!(
                "'{}' is not a skill in the registry",
                name
            )));
        }
        Ok(entry)
    }

    async fn resolve_skill_artifact(
        &self,
        target: &str,
        version: Option<&str>,
        absolute_only: bool,
    ) -> Result<
        (
            String,
            Vec<u8>,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
        ),
        PackageError,
    > {
        let (name, bytes, source, description, author, resolved_version) = if target
            .starts_with("http://")
            || target.starts_with("https://")
        {
            let url = target.to_string();
            let name = url
                .rsplit('/')
                .next()
                .unwrap_or("skill")
                .replace(".tar.gz", "")
                .replace(".zip", "");
            let bytes = reqwest::get(&url).await?.bytes().await?.to_vec();
            (name, bytes, url, None, None, None)
        } else if (!absolute_only || Path::new(target).is_absolute()) && Path::new(target).exists()
        {
            let path = Path::new(target);
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("skill")
                .to_string();
            let bytes = std::fs::read(path)?;
            (name, bytes, target.to_string(), None, None, None)
        } else {
            let entry = self.fetch_skill_entry(target).await?;
            let ver = version.unwrap_or_else(|| entry["latest"].as_str().unwrap_or("0.1.0"));
            let url = entry
                .get("versions")
                .and_then(|v| v.get(ver))
                .and_then(|vo| vo.get("url"))
                .and_then(|u| u.as_str())
                .ok_or_else(|| {
                    PackageError::NotFound(format!("No artifact for skill '{}' (v{})", target, ver))
                })?;
            let description = entry
                .get("description")
                .and_then(|v| v.as_str())
                .map(ToString::to_string);
            let author = entry
                .get("author")
                .and_then(|v| v.as_str())
                .map(ToString::to_string);
            let bytes = reqwest::get(url).await?.bytes().await?.to_vec();
            (
                target.to_string(),
                bytes,
                "registry".to_string(),
                description,
                author,
                Some(ver.to_string()),
            )
        };

        Ok((name, bytes, source, description, author, resolved_version))
    }

    pub async fn install_skill(
        &self,
        target: &str,
        version: Option<String>,
    ) -> Result<InstalledSkill, PackageError> {
        self.install_skill_internal(target, version, false, false)
            .await
            .map(|(skill, _)| skill)
    }

    pub async fn install_new_skill(
        &self,
        target: &str,
        version: Option<String>,
    ) -> Result<InstalledSkill, PackageError> {
        self.install_skill_internal(target, version, true, false)
            .await
            .map(|(skill, _)| skill)
    }

    pub async fn update_skill_with_backup(
        &self,
        target: &str,
        version: Option<String>,
    ) -> Result<(InstalledSkill, Option<PathBuf>), PackageError> {
        self.install_skill_internal(target, version, false, true)
            .await
    }

    async fn install_skill_internal(
        &self,
        target: &str,
        version: Option<String>,
        new_only: bool,
        backup_modified: bool,
    ) -> Result<(InstalledSkill, Option<PathBuf>), PackageError> {
        let _lock = PackageLock::acquire(&self.base_dir)?;
        let original_records = record_bytes(&self.lockfile_path)?;
        let existing: SkillLockfile = read_install_records(&self.lockfile_path)?;
        let (name, bytes, source, description, author, resolved_version) = self
            .resolve_skill_artifact(target, version.as_deref(), new_only || backup_modified)
            .await?;

        let skills_dir = self.base_dir.join("skills");
        std::fs::create_dir_all(&skills_dir)?;
        if std::fs::symlink_metadata(&skills_dir)?
            .file_type()
            .is_symlink()
        {
            return Err(PackageError::InvalidSpec(
                "Skill storage is a symbolic link; existing files were preserved".into(),
            ));
        }

        let dest = skills_dir.join(&name);
        sanitize_path_component(&name, "skill name")?;
        if backup_modified && !existing.skills.contains_key(&name) {
            return Err(PackageError::InvalidSpec(
                "Skill is no longer installed".into(),
            ));
        }
        if new_only && (existing.skills.contains_key(&name) || dest.exists()) {
            return Err(PackageError::InvalidSpec(
                "Skill already exists; existing files were preserved".into(),
            ));
        }
        let backup = if backup_modified && dest.exists() {
            let installed = existing.skills.get(&name).ok_or_else(|| {
                PackageError::InvalidSpec(
                    "Skill directory is not registered; existing files were preserved".into(),
                )
            })?;
            backup_modified_skill(&dest, installed)?
        } else {
            None
        };
        if dest.exists() {
            std::fs::remove_dir_all(&dest)?;
        }
        std::fs::create_dir_all(&dest)?;

        let _ = extract_skill_bundle(&bytes, &dest, &name)?;

        let version =
            version.unwrap_or_else(|| resolved_version.unwrap_or_else(|| "0.1.0".to_string()));

        let files = walk_skill_files(&dest, &name)?;

        let installed = InstalledSkill {
            name: name.clone(),
            description,
            version,
            author,
            source,
            files,
        };

        if record_bytes(&self.lockfile_path)? != original_records {
            return Err(PackageError::InvalidSpec(
                "Package records changed during installation; existing records were preserved"
                    .into(),
            ));
        }
        let mut lockfile = existing;
        lockfile.skills.insert(name.clone(), installed.clone());
        self.save_skills_lockfile(&lockfile)?;

        Ok((installed, backup))
    }

    pub fn uninstall_skill(&self, name: &str) -> Result<bool, PackageError> {
        let _lock = PackageLock::acquire(&self.base_dir)?;
        let mut lockfile: SkillLockfile = read_install_records(&self.lockfile_path)?;
        if let Some(skill) = lockfile.skills.remove(name) {
            sanitize_path_component(&skill.name, "skill name")?;
            if std::fs::symlink_metadata(self.base_dir.join("skills"))?
                .file_type()
                .is_symlink()
            {
                return Err(PackageError::InvalidSpec(
                    "Skill storage is a symbolic link".into(),
                ));
            }
            let dir = self.base_dir.join("skills").join(&skill.name);
            if dir.exists() {
                std::fs::remove_dir_all(&dir)?;
            }
            self.save_skills_lockfile(&lockfile)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub async fn check_skill_updates(
        &self,
        names: Option<&[String]>,
    ) -> Result<Vec<PluginUpdateStatus>, PackageError> {
        let lockfile = self.load_skills_lockfile();
        let registry = self.fetch_skills().await?;

        let mut statuses = Vec::new();
        for (name, installed) in &lockfile.skills {
            if let Some(filter) = names
                && !filter.iter().any(|n| n.eq_ignore_ascii_case(name))
            {
                continue;
            }
            if let Some(reg) = registry.iter().find(|s| s.name == *name) {
                let has_update = is_newer_version(&reg.latest, &installed.version);
                statuses.push(PluginUpdateStatus {
                    name: name.clone(),
                    has_wasm: false,
                    has_native: false,
                    installed_version: installed.version.clone(),
                    latest_version: reg.latest.clone(),
                    has_update,
                });
            }
        }
        statuses.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(statuses)
    }
}

fn is_native_archive_or_binary(target: &str) -> bool {
    target.ends_with(".zip")
        || target.ends_with(".tar.gz")
        || target.ends_with(".tgz")
        || (cfg!(windows) && target.ends_with(".exe"))
        || (!target.ends_with(".wasm") && Path::new(target).is_file())
}

fn extract_skill_bundle(
    bytes: &[u8],
    dest_dir: &Path,
    default_name: &str,
) -> Result<PathBuf, PackageError> {
    if bytes.starts_with(b"PK\x03\x04") {
        let reader = Cursor::new(bytes);
        let mut zip = zip::ZipArchive::new(reader)
            .map_err(|e| PackageError::InvalidSpec(format!("Failed to read zip archive: {}", e)))?;

        for i in 0..zip.len() {
            let mut file = zip
                .by_index(i)
                .map_err(|e| PackageError::InvalidSpec(format!("Zip entry error: {}", e)))?;
            let enclosed = file.enclosed_name().ok_or_else(|| {
                PackageError::InvalidSpec("Invalid zip entry path traversal".to_string())
            })?;
            let outpath = dest_dir.join(enclosed);

            if file.is_dir() {
                std::fs::create_dir_all(&outpath)?;
            } else {
                if let Some(p) = outpath.parent() {
                    std::fs::create_dir_all(p)?;
                }
                let mut outfile = std::fs::File::create(&outpath)?;
                std::io::copy(&mut file, &mut outfile)?;
            }
        }
        Ok(dest_dir.to_path_buf())
    } else if bytes.starts_with(b"\x1f\x8b") {
        let gz = GzDecoder::new(bytes);
        let mut archive = Archive::new(gz);
        unpack_tar_safe(&mut archive, dest_dir)?;
        Ok(dest_dir.to_path_buf())
    } else {
        let file_name = default_name.to_string();
        let outpath = dest_dir.join(&file_name);
        std::fs::write(&outpath, bytes)?;
        Ok(outpath)
    }
}

fn walk_skill_files(root: &Path, _skill_name: &str) -> Result<Vec<SkillFile>, PackageError> {
    let mut files = Vec::new();
    if root.exists() {
        collect_skill_files(root, root, &mut files)?;
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

fn backup_modified_skill(
    root: &Path,
    installed: &InstalledSkill,
) -> Result<Option<PathBuf>, PackageError> {
    fn copy_tree(source: &Path, target: &Path) -> Result<(), PackageError> {
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            let destination = target.join(entry.file_name());
            if kind.is_symlink() {
                return Err(PackageError::InvalidSpec(
                    "Cannot back up a skill containing symbolic links".into(),
                ));
            }
            if kind.is_dir() {
                std::fs::create_dir(&destination)?;
                copy_tree(&entry.path(), &destination)?;
            } else if kind.is_file() {
                std::fs::copy(entry.path(), destination)?;
            } else {
                return Err(PackageError::InvalidSpec(
                    "Cannot back up a skill containing special files".into(),
                ));
            }
        }
        Ok(())
    }
    // Refuse symbolic links before walking, so neither inspection nor backup follows them.
    fn check_tree(dir: &Path) -> Result<(), PackageError> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_symlink() || (!kind.is_dir() && !kind.is_file()) {
                return Err(PackageError::InvalidSpec(
                    "Cannot update a skill containing links or special files".into(),
                ));
            }
            if kind.is_dir() {
                check_tree(&entry.path())?;
            }
        }
        Ok(())
    }
    if std::fs::symlink_metadata(root)?.file_type().is_symlink() {
        return Err(PackageError::InvalidSpec(
            "Cannot update a linked skill directory".into(),
        ));
    }
    check_tree(root)?;
    let current = walk_skill_files(root, &installed.name)?;
    let unchanged = current.len() == installed.files.len()
        && current.iter().all(|file| {
            installed.files.iter().any(|original| {
                original.path == file.path
                    && original.sha256.is_some()
                    && original.sha256 == file.sha256
            })
        });
    if unchanged {
        return Ok(None);
    }
    let parent = root
        .parent()
        .ok_or_else(|| PackageError::InvalidSpec("Invalid skill directory".into()))?;
    let backup = parent.join(format!("{}.backup-{}", installed.name, chrono_timestamp()));
    std::fs::create_dir(&backup)?;
    copy_tree(root, &backup)?;
    Ok(Some(backup))
}

fn collect_skill_files(
    dir: &Path,
    root: &Path,
    out: &mut Vec<SkillFile>,
) -> Result<(), PackageError> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let rel = path.strip_prefix(root).unwrap_or(&path);
        if path.is_dir() {
            collect_skill_files(&path, root, out)?;
        } else {
            let mut hasher = Sha256::new();
            hasher.update(std::fs::read(&path)?);
            let sha = hasher
                .finalize()
                .iter()
                .map(|b| format!("{:02x}", b))
                .collect::<String>();
            out.push(SkillFile {
                path: rel.to_string_lossy().to_string(),
                sha256: Some(sha),
            });
        }
    }
    Ok(())
}

fn sanitize_path_component(value: &str, label: &str) -> Result<(), PackageError> {
    if value.is_empty() || value.contains(['/', '\\']) || value.contains("..") {
        return Err(PackageError::InvalidSpec(format!(
            "Invalid {}: '{}' must not contain path separators",
            label, value
        )));
    }
    Ok(())
}

pub fn is_newer_version(candidate: &str, current: &str) -> bool {
    let parse = |v: &str| -> (u64, u64, u64) {
        let trimmed = v.trim().strip_prefix('v').unwrap_or(v.trim());
        let mut parts = trimmed.split('.');
        let major = parts
            .next()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        let minor = parts
            .next()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        let patch = parts
            .next()
            .and_then(|s| s.split('-').next())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        (major, minor, patch)
    };
    parse(candidate) > parse(current)
}

fn current_target_triple() -> &'static str {
    #[cfg(all(target_os = "windows", target_arch = "x86_64", target_env = "gnu"))]
    return "x86_64-pc-windows-gnu";

    #[cfg(all(target_os = "windows", target_arch = "x86_64", not(target_env = "gnu")))]
    return "x86_64-pc-windows-msvc";

    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    return "aarch64-pc-windows-msvc";

    #[cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "musl"))]
    return "x86_64-unknown-linux-musl";

    #[cfg(all(target_os = "linux", target_arch = "x86_64", not(target_env = "musl")))]
    return "x86_64-unknown-linux-gnu";

    #[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "musl"))]
    return "aarch64-unknown-linux-musl";

    #[cfg(all(target_os = "linux", target_arch = "aarch64", not(target_env = "musl")))]
    return "aarch64-unknown-linux-gnu";

    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    return "x86_64-apple-darwin";

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return "aarch64-apple-darwin";

    #[allow(unreachable_code)]
    "unknown"
}

fn extract_single_binary(bytes: &[u8], binary_name: &str) -> Result<Vec<u8>, PackageError> {
    use std::io::Read;

    let clean_binary = binary_name.to_ascii_lowercase();
    let mut expected_names = vec![clean_binary.clone(), format!("{}.exe", clean_binary)];
    // Existing releases used rune/rune.exe. Keep them readable by the renamed CLI.
    if clean_binary == "rune-kit" {
        expected_names.extend(["rune".to_string(), "rune.exe".to_string()]);
    }

    let matches_binary = |fname: &str| -> bool {
        let lower = fname.to_ascii_lowercase().replace('\\', "/");
        let filename = lower.split('/').next_back().unwrap_or("");
        expected_names.iter().any(|target| filename == target)
    };

    if bytes.starts_with(b"PK\x03\x04") {
        let reader = Cursor::new(bytes);
        let mut zip = zip::ZipArchive::new(reader)
            .map_err(|e| PackageError::InvalidSpec(format!("Zip read error: {}", e)))?;

        for i in 0..zip.len() {
            let mut file = zip
                .by_index(i)
                .map_err(|e| PackageError::InvalidSpec(format!("Zip entry error: {}", e)))?;
            if matches_binary(file.name()) {
                let mut buf = Vec::new();
                file.read_to_end(&mut buf)?;
                return Ok(buf);
            }
        }

        // Fallback: single file in archive
        let mut non_dir_files = Vec::new();
        for i in 0..zip.len() {
            let file = zip
                .by_index(i)
                .map_err(|e| PackageError::InvalidSpec(format!("Zip entry error: {}", e)))?;
            if !file.is_dir() {
                non_dir_files.push(i);
            }
        }
        if non_dir_files.len() == 1 {
            let mut file = zip
                .by_index(non_dir_files[0])
                .map_err(|e| PackageError::InvalidSpec(format!("Zip entry error: {}", e)))?;
            let mut buf = Vec::new();
            file.read_to_end(&mut buf)?;
            return Ok(buf);
        }
    } else if bytes.starts_with(b"\x1f\x8b") {
        let gz = GzDecoder::new(bytes);
        let mut archive = Archive::new(gz);
        let entries = archive.entries()?;
        let mut single_buf = None;
        let mut file_count = 0;

        for entry_res in entries {
            let mut entry = entry_res?;
            let path_str = entry.path()?.to_string_lossy().to_string();
            let is_file = entry.header().entry_type().is_file();

            if is_file {
                file_count += 1;
                let mut buf = Vec::new();
                entry.read_to_end(&mut buf)?;
                if matches_binary(&path_str) {
                    return Ok(buf);
                }
                single_buf = Some(buf);
            }
        }
        if file_count == 1
            && let Some(buf) = single_buf
        {
            return Ok(buf);
        }
    } else {
        return Ok(bytes.to_vec());
    }

    Err(PackageError::SelfUpdate(format!(
        "Binary '{}' not found inside downloaded archive",
        binary_name
    )))
}

fn unpack_tar_safe<R: std::io::Read>(
    archive: &mut Archive<R>,
    dest_dir: &Path,
) -> Result<(), PackageError> {
    for entry_res in archive.entries()? {
        let mut entry = entry_res?;
        let path = entry.path()?;

        let mut normalized = PathBuf::new();
        for comp in path.components() {
            match comp {
                std::path::Component::Normal(c) => normalized.push(c),
                std::path::Component::CurDir => {}
                _ => {
                    return Err(PackageError::InvalidSpec(format!(
                        "Path traversal detected in tar archive: {:?}",
                        path
                    )));
                }
            }
        }

        if normalized.as_os_str().is_empty() {
            continue;
        }

        let outpath = dest_dir.join(&normalized);
        if entry.header().entry_type().is_dir() {
            std::fs::create_dir_all(&outpath)?;
        } else {
            if let Some(parent) = outpath.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut outfile = std::fs::File::create(&outpath)?;
            std::io::copy(&mut entry, &mut outfile)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(mode) = entry.header().mode() {
                    let _ =
                        std::fs::set_permissions(&outpath, std::fs::Permissions::from_mode(mode));
                }
            }
        }
    }
    Ok(())
}

fn extract_archive_or_binary(
    bytes: &[u8],
    dest_dir: &Path,
    default_name: &str,
) -> Result<PathBuf, PackageError> {
    if bytes.starts_with(b"PK\x03\x04") {
        let reader = Cursor::new(bytes);
        let mut zip = zip::ZipArchive::new(reader)
            .map_err(|e| PackageError::InvalidSpec(format!("Zip read error: {}", e)))?;

        for i in 0..zip.len() {
            let mut file = zip
                .by_index(i)
                .map_err(|e| PackageError::InvalidSpec(format!("Zip entry error: {}", e)))?;
            let enclosed = file.enclosed_name().ok_or_else(|| {
                PackageError::InvalidSpec("Invalid zip entry path traversal".to_string())
            })?;
            let outpath = dest_dir.join(enclosed);

            if file.is_dir() {
                std::fs::create_dir_all(&outpath)?;
            } else {
                if let Some(p) = outpath.parent() {
                    std::fs::create_dir_all(p)?;
                }
                let mut outfile = std::fs::File::create(&outpath)?;
                std::io::copy(&mut file, &mut outfile)?;
            }
        }
    } else if bytes.starts_with(b"\x1f\x8b") {
        let gz = GzDecoder::new(bytes);
        let mut archive = Archive::new(gz);
        unpack_tar_safe(&mut archive, dest_dir)?;
    } else {
        let binary_file_name = if cfg!(windows) {
            format!("{}.exe", default_name)
        } else {
            default_name.to_string()
        };
        let outpath = dest_dir.join(&binary_file_name);
        std::fs::write(&outpath, bytes)?;
        return Ok(outpath);
    }

    find_executable_in_dir(dest_dir, default_name)
}

fn find_executable_in_dir(dir: &Path, expected_name: &str) -> Result<PathBuf, PackageError> {
    let mut candidates = Vec::new();
    collect_executables_in_dir(dir, &mut candidates)?;

    let clean_name = expected_name.to_ascii_lowercase();
    let native_name = format!("{}-native", clean_name);
    let stripped_name = clean_name
        .strip_prefix("rune-")
        .unwrap_or(&clean_name)
        .to_string();
    let stripped_native = format!("{}-native", stripped_name);

    // 1. Exact stem match
    for c in &candidates {
        let stem = c
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if stem == clean_name {
            return Ok(c.clone());
        }
    }

    // 2. Native sidecar stem match (e.g. "rune-browser-native", "browser-native", "browser")
    for c in &candidates {
        let stem = c
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if stem == native_name || stem == stripped_native || stem == stripped_name {
            return Ok(c.clone());
        }
    }

    // 3. Starts with match (e.g. "rune-browser-x86_64-pc-windows-msvc")
    for c in &candidates {
        let stem = c
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if stem.starts_with(&clean_name) || stem.starts_with(&stripped_name) {
            return Ok(c.clone());
        }
    }

    // 4. Fallback: single executable in archive
    if candidates.len() == 1 {
        return Ok(candidates[0].clone());
    }

    Err(PackageError::InvalidSpec(format!(
        "No executable found in '{}' matching '{}'",
        dir.display(),
        expected_name
    )))
}

fn collect_executables_in_dir(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), std::io::Error> {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_executables_in_dir(&path, out)?;
            } else if path.is_file() {
                #[cfg(windows)]
                {
                    if path
                        .extension()
                        .and_then(|s| s.to_str())
                        .map(|e| e.eq_ignore_ascii_case("exe"))
                        .unwrap_or(false)
                    {
                        out.push(path);
                    }
                }
                #[cfg(not(windows))]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Ok(meta) = path.metadata()
                        && meta.permissions().mode() & 0o111 != 0
                    {
                        out.push(path);
                    }
                }
            }
        }
    }
    Ok(())
}

fn set_executable_permissions(_path: &Path) -> Result<(), std::io::Error> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = std::fs::metadata(_path) {
            let mut perms = metadata.permissions();
            perms.set_mode(perms.mode() | 0o755);
            std::fs::set_permissions(_path, perms)?;
        }
    }
    Ok(())
}

fn chrono_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod cli_archive_tests {
    use super::*;
    use std::io::Write;

    #[tokio::test]
    async fn skill_update_backs_up_modified_files_beside_the_skill() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("kit");
        std::fs::create_dir_all(&root).unwrap();
        let source = temp.path().join("example.md");
        std::fs::write(&source, b"original").unwrap();
        let manager = SkillManager {
            base_dir: root.clone(),
            lockfile_path: root.join("installed_skills.json"),
        };
        manager
            .install_new_skill(source.to_str().unwrap(), None)
            .await
            .unwrap();
        let dir = root.join("skills/example");
        let mut files = walk_skill_files(&dir, "example").unwrap();
        assert_eq!(files.len(), 1);
        let installed_file = dir.join(files.remove(0).path);
        std::fs::write(&installed_file, b"user edit").unwrap();
        std::fs::write(&source, b"updated").unwrap();
        let (_, backup) = manager
            .update_skill_with_backup(source.to_str().unwrap(), None)
            .await
            .unwrap();
        let backup = backup.unwrap();
        assert_eq!(backup.parent(), dir.parent());
        assert_eq!(
            std::fs::read(backup.join(installed_file.file_name().unwrap())).unwrap(),
            b"user edit"
        );
        assert_eq!(std::fs::read(installed_file).unwrap(), b"updated");
        assert!(manager.uninstall_skill("example").unwrap());
        assert!(backup.exists());
    }

    #[tokio::test]
    async fn registry_download_checks_http_status_and_integrity_before_returning_payload() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for (status, payload, success) in [
            ("200 OK", "plugin", true),
            ("200 OK", "broken", false),
            ("200 OK", "short", false),
            ("404 Not Found", "plugin", false),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}/plugin.wasm", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    socket.read_exact(&mut byte).await.unwrap();
                    request.push(byte[0]);
                    assert!(request.len() < 4096);
                }
                let reply = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                    payload.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            });
            let checksum = crate::registry::ArtifactChecksum {
                sha256: Sha256::digest(b"plugin")
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>(),
                size: 6,
            };
            let client = reqwest::Client::builder().no_proxy().build().unwrap();
            let result = download_plugin(&client, &url, &checksum).await;
            assert_eq!(result.is_ok(), success, "{status}: {payload}: {result:?}");
            server.await.unwrap();
        }
    }

    #[test]
    fn self_update_accepts_new_and_legacy_binary_names() {
        for name in ["rune-kit", "rune-kit.exe", "rune", "rune.exe"] {
            let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
            let options = zip::write::SimpleFileOptions::default();
            writer.start_file("README.txt", options).unwrap();
            writer.write_all(b"not the binary").unwrap();
            writer.start_file(name, options).unwrap();
            writer.write_all(b"expected executable").unwrap();
            let bytes = writer.finish().unwrap().into_inner();
            assert_eq!(
                extract_single_binary(&bytes, "rune-kit").unwrap(),
                b"expected executable"
            );
        }
    }
}

#[cfg(test)]
mod hidden_root_update_tests {
    use super::*;
    #[test]
    fn self_update_in_hidden_root_verifies_checksum_and_preserves_infer_and_tools() {
        let root = std::env::temp_dir().join(format!("kit-update-{}", std::process::id()));
        let hidden = root.join(".rune");
        std::fs::create_dir_all(hidden.join("tools/plugins")).unwrap();
        std::fs::create_dir_all(hidden.join("infer/config")).unwrap();
        let exe = hidden.join(if cfg!(windows) {
            "rune-kit.exe"
        } else {
            "rune-kit"
        });
        let infer = hidden.join(if cfg!(windows) {
            "rune-infer.exe"
        } else {
            "rune-infer"
        });
        std::fs::write(&exe, b"previous kit").unwrap();
        std::fs::write(&infer, b"infer").unwrap();
        std::fs::write(hidden.join("tools/plugins/test.wasm"), b"plugin").unwrap();
        let update = b"verified new executable";
        let digest: String = Sha256::digest(update)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert!(verify_cli_checksum(update, &"0".repeat(64)).is_err());
        assert_eq!(std::fs::read(&exe).unwrap(), b"previous kit");
        verify_cli_checksum(update, &digest).unwrap();
        activate_cli_update(&exe, update).unwrap();
        activate_cli_update(&exe, update).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), update);
        assert_eq!(std::fs::read(&infer).unwrap(), b"infer");
        assert_eq!(
            std::fs::read(hidden.join("tools/plugins/test.wasm")).unwrap(),
            b"plugin"
        );
        assert!(hidden.join("infer/config").is_dir());
        std::fs::remove_dir_all(root).unwrap();
    }
}
