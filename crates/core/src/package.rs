use crate::manifest::{
    ExecutionKind, InstalledPlugin, InstalledSkill, Lockfile, PluginUpdateStatus,
    RegistryPluginSummary, RegistrySkillSummary, SkillFile, SkillLockfile,
};
use crate::runtime::{NativeSidecar, WasmPluginInstance};
use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use tar::Archive;
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
    #[error("Runtime initialization probe failed: {0}")]
    Runtime(#[from] crate::runtime::RuntimeError),
    #[error("Self-update error: {0}")]
    SelfUpdate(String),
}

pub struct FetchedBundle {
    pub name: String,
    pub version: Option<String>,
    pub wasm_bytes: Option<Vec<u8>>,
    pub native_bytes: Option<Vec<u8>>,
    pub source: String,
}

pub struct PackageManager {
    base_dir: PathBuf,
    lockfile_path: PathBuf,
}

impl Default for PackageManager {
    fn default() -> Self {
        Self::new()
    }
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
        std::fs::write(&self.lockfile_path, content)
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
        let index: serde_json::Value = client.get(registry_url).send().await?.json().await?;

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
        requested_version: Option<&str>,
    ) -> Result<(String, String), PackageError> {
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

        Ok((tag_name, download_url))
    }

    pub async fn self_update(
        &self,
        requested_version: Option<&str>,
        force: bool,
    ) -> Result<String, PackageError> {
        let current_ver = env!("CARGO_PKG_VERSION");
        let (tag, download_url) = self.fetch_latest_cli_release(requested_version).await?;
        let target_ver = tag.strip_prefix('v').unwrap_or(&tag);

        if !force && !is_newer_version(target_ver, current_ver) && target_ver == current_ver {
            return Err(PackageError::SelfUpdate(format!(
                "Already on the latest version (v{}). Use --force to overwrite.",
                current_ver
            )));
        }

        let client = reqwest::Client::builder().user_agent("rune-kit").build()?;
        let archive_bytes = client
            .get(&download_url)
            .send()
            .await?
            .bytes()
            .await?
            .to_vec();

        let current_exe = std::env::current_exe()?;
        let exe_dir = current_exe.parent().ok_or_else(|| {
            PackageError::SelfUpdate("Could not determine current executable directory".to_string())
        })?;

        let binary_bytes = extract_single_binary(&archive_bytes, "rune")?;
        let temp_bin = exe_dir.join(format!(".rune-update-{}.tmp", chrono_timestamp()));
        std::fs::write(&temp_bin, &binary_bytes)?;
        set_executable_permissions(&temp_bin)?;

        #[cfg(windows)]
        {
            let old_backup = exe_dir.join(format!(".rune-old-{}.tmp", chrono_timestamp()));
            std::fs::rename(&current_exe, &old_backup)?;
            if let Err(e) = std::fs::rename(&temp_bin, &current_exe) {
                let _ = std::fs::rename(&old_backup, &current_exe);
                return Err(PackageError::Io(e));
            }
            let _ = std::fs::remove_file(&old_backup);
        }

        #[cfg(not(windows))]
        {
            std::fs::rename(&temp_bin, &current_exe)?;
        }

        Ok(tag)
    }

    pub async fn install(
        &self,
        target: &str,
        version: Option<String>,
        prefer_native: bool,
    ) -> Result<InstalledPlugin, PackageError> {
        let bundle = if target.ends_with(".wasm") && Path::new(target).exists() {
            let path = Path::new(target);
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| {
                    PackageError::InvalidSpec(format!(
                        "Cannot derive a plugin name from path '{}'",
                        target
                    ))
                })?
                .to_string();
            let bytes = std::fs::read(path)?;
            FetchedBundle {
                name: stem,
                version,
                wasm_bytes: Some(bytes),
                native_bytes: None,
                source: target.to_string(),
            }
        } else if is_native_archive_or_binary(target) && Path::new(target).exists() {
            let path = Path::new(target);
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("plugin")
                .to_string();
            let bytes = std::fs::read(path)?;
            FetchedBundle {
                name: stem,
                version,
                wasm_bytes: None,
                native_bytes: Some(bytes),
                source: target.to_string(),
            }
        } else if target.starts_with("http://") || target.starts_with("https://") {
            let res = reqwest::get(target).await?.bytes().await?.to_vec();
            let name = target
                .split('/')
                .next_back()
                .unwrap_or("plugin")
                .replace(".wasm", "")
                .replace(".zip", "")
                .replace(".tar.gz", "");

            if target.ends_with(".wasm") {
                FetchedBundle {
                    name,
                    version,
                    wasm_bytes: Some(res),
                    native_bytes: None,
                    source: target.to_string(),
                }
            } else {
                FetchedBundle {
                    name,
                    version,
                    wasm_bytes: None,
                    native_bytes: Some(res),
                    source: target.to_string(),
                }
            }
        } else {
            self.fetch_from_registry(target, version.clone(), prefer_native)
                .await?
        };

        let plugins_dir = self.base_dir.join("plugins");
        std::fs::create_dir_all(&plugins_dir)?;

        let mut probe_params = HashMap::new();
        probe_params.insert("allowed_hosts".to_string(), String::new());

        let (mut final_name, mut final_ver, mut final_desc) = (
            bundle.name.clone(),
            bundle
                .version
                .clone()
                .unwrap_or_else(|| "0.1.0".to_string()),
            None,
        );

        if let Some(ref wasm_bytes) = bundle.wasm_bytes
            && let Ok(mut instance) = WasmPluginInstance::load_from_bytes(
                &bundle.name,
                wasm_bytes.clone(),
                probe_params.clone(),
            )
            && let Ok(info) = instance.get_info()
        {
            final_name = if bundle.name.is_empty() {
                info.name
            } else {
                bundle.name.clone()
            };
            final_ver = bundle.version.clone().unwrap_or(info.version);
            final_desc = info.description;
        }

        sanitize_path_component(&final_name, "plugin name")?;
        sanitize_path_component(&final_ver, "plugin version")?;

        let mut wasm_rel_path = None;
        let mut sha256_hash = String::new();

        if let Some(ref wasm_bytes) = bundle.wasm_bytes {
            sha256_hash = Sha256::digest(wasm_bytes)
                .iter()
                .map(|b| format!("{:02x}", b))
                .collect();

            let file_name = format!("{}-{}.wasm", final_name, final_ver);
            let dest = plugins_dir.join(&file_name);
            std::fs::write(&dest, wasm_bytes)?;
            wasm_rel_path = Some(format!("plugins/{}", file_name));
        }

        let mut native_rel_path = None;

        if let Some(ref native_bytes) = bundle.native_bytes {
            if sha256_hash.is_empty() {
                sha256_hash = Sha256::digest(native_bytes)
                    .iter()
                    .map(|b| format!("{:02x}", b))
                    .collect();
            }

            let temp_dir_name = format!(".tmp-{}-{}", final_name, chrono_timestamp());
            let temp_extract_dir = plugins_dir.join(&temp_dir_name);
            std::fs::create_dir_all(&temp_extract_dir)?;

            let extracted_binary =
                extract_archive_or_binary(native_bytes, &temp_extract_dir, &final_name)?;
            set_executable_permissions(&extracted_binary)?;

            if final_desc.is_none()
                && let Ok(mut sidecar) =
                    NativeSidecar::new(&final_name, &extracted_binary, probe_params)
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
            if final_bin_path.exists() {
                let _ = std::fs::remove_file(&final_bin_path);
            }

            if std::fs::rename(&extracted_binary, &final_bin_path).is_err() {
                std::fs::copy(&extracted_binary, &final_bin_path)?;
                let _ = std::fs::remove_file(&extracted_binary);
            }

            set_executable_permissions(&final_bin_path)?;
            let _ = std::fs::remove_dir_all(&temp_extract_dir);

            native_rel_path = Some(format!("plugins/{}", bin_file_name));
        }

        let execution_kind = if prefer_native && native_rel_path.is_some() {
            ExecutionKind::Native
        } else if wasm_rel_path.is_some() {
            ExecutionKind::Wasm
        } else {
            ExecutionKind::Native
        };

        let primary_binary_path = wasm_rel_path
            .clone()
            .or_else(|| native_rel_path.clone())
            .ok_or_else(|| {
                PackageError::InvalidSpec("Neither WASM nor Native binary was saved".to_string())
            })?;

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
        };

        let mut lockfile = self.load_lockfile();
        lockfile.plugins.insert(final_name, installed.clone());
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
            if let Some(ref native_rel) = plugin.native_binary_path {
                let native_bin = self.base_dir.join(native_rel);
                if native_bin.exists() {
                    let _ = std::fs::remove_file(native_bin);
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
        let index: serde_json::Value = client.get(registry_url).send().await?.json().await?;

        let pkg = index
            .get(name)
            .ok_or_else(|| PackageError::NotFound(name.to_string()))?;

        let ver = version.unwrap_or_else(|| pkg["latest"].as_str().unwrap_or("0.1.0").to_string());
        let ver_obj = &pkg["versions"][&ver];
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

        let wasm_bytes = if let Some(url) = wasm_url {
            let bytes = client.get(url).send().await?.bytes().await?.to_vec();
            println!("Downloaded wasm build {}", format_bytes(bytes.len()));
            Some(bytes)
        } else {
            None
        };

        let native_bytes = if let Some(url) = native_url {
            let bytes = client.get(url).send().await?.bytes().await?.to_vec();
            println!("Downloaded native build {}", format_bytes(bytes.len()));
            Some(bytes)
        } else {
            if prefer_native {
                eprintln!(
                    "[warn] No native build available for host target triple '{}' on plugin '{}'. Falling back to WASM.",
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
            source: "registry".to_string(),
        })
    }
}

// ---------------------------------------------------------------------------
// Skill management
// ---------------------------------------------------------------------------

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
        let base_dir = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("rune-kit");

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
        std::fs::write(&self.lockfile_path, content)
    }

    pub async fn fetch_skills(&self) -> Result<Vec<RegistrySkillSummary>, PackageError> {
        let skills_url = "https://raw.githubusercontent.com/shba007/rune-tools/refs/heads/main/registry/index.json";
        let client = reqwest::Client::new();
        let index: serde_json::Value = client.get(skills_url).send().await?.json().await?;

        let mut results = Vec::new();
        if let Some(map) = index.as_object() {
            for (name, skill) in map {
                let latest = skill
                    .get("latest")
                    .and_then(|v| v.as_str())
                    .unwrap_or("0.1.0")
                    .to_string();
                let description = skill
                    .get("description")
                    .and_then(|v| v.as_str())
                    .map(ToString::to_string);
                let author = skill
                    .get("author")
                    .and_then(|v| v.as_str())
                    .map(ToString::to_string);
                let tags = skill
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
        index
            .get(name)
            .cloned()
            .ok_or_else(|| PackageError::NotFound(name.to_string()))
    }

    async fn resolve_skill_artifact(
        &self,
        target: &str,
        version: Option<&str>,
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
        } else if Path::new(target).exists() {
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
        let (name, bytes, source, description, author, resolved_version) = self
            .resolve_skill_artifact(target, version.as_deref())
            .await?;

        let skills_dir = self.base_dir.join("skills");
        std::fs::create_dir_all(&skills_dir)?;

        let dest = skills_dir.join(&name);
        if dest.exists() {
            let _ = std::fs::remove_dir_all(&dest);
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

        let mut lockfile = self.load_skills_lockfile();
        lockfile.skills.insert(name.clone(), installed.clone());
        self.save_skills_lockfile(&lockfile)?;

        Ok(installed)
    }

    pub fn uninstall_skill(&self, name: &str) -> Result<bool, PackageError> {
        let mut lockfile = self.load_skills_lockfile();
        if let Some(skill) = lockfile.skills.remove(name) {
            let dir = self.base_dir.join("skills").join(&skill.name);
            let _ = std::fs::remove_dir_all(&dir);
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
        archive.unpack(dest_dir)?;
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

#[cfg(windows)]
fn current_target_triple() -> &'static str {
    "x86_64-pc-windows-msvc"
}

#[cfg(not(windows))]
fn current_target_triple() -> &'static str {
    std::env::consts::TARGET.as_str()
}

fn is_native_archive_or_binary(target: &str) -> bool {
    target.ends_with(".zip")
        || target.ends_with(".tar.gz")
        || target.ends_with(".tgz")
        || (cfg!(windows) && target.ends_with(".exe"))
        || (!target.ends_with(".wasm") && Path::new(target).is_file())
}

fn extract_single_binary(bytes: &[u8], binary_name: &str) -> Result<Vec<u8>, PackageError> {
    use std::io::Read;
    if bytes.starts_with(b"PK\x03\x04") {
        let reader = Cursor::new(bytes);
        let mut zip = zip::ZipArchive::new(reader)
            .map_err(|e| PackageError::InvalidSpec(format!("Zip read error: {}", e)))?;
        for i in 0..zip.len() {
            let mut file = zip
                .by_index(i)
                .map_err(|e| PackageError::InvalidSpec(format!("Zip entry error: {}", e)))?;
            let fname = file.name().to_string();
            if fname == binary_name || fname.ends_with(&format!("/{}", binary_name)) {
                let mut buf = Vec::new();
                file.read_to_end(&mut buf)?;
                return Ok(buf);
            }
        }
    } else if bytes.starts_with(b"\x1f\x8b") {
        let gz = GzDecoder::new(bytes);
        let mut archive = Archive::new(gz);
        for entry in archive.entries()? {
            let mut entry = entry?;
            let path = entry.path()?;
            let fname = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if fname == binary_name {
                let mut buf = Vec::new();
                entry.read_to_end(&mut buf)?;
                return Ok(buf);
            }
        }
    } else {
        return Ok(bytes.to_vec());
    }

    Err(PackageError::SelfUpdate(format!(
        "Binary '{}' not found inside downloaded archive",
        binary_name
    )))
}

fn extract_archive_or_binary(
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
    } else if bytes.starts_with(b"\x1f\x8b") {
        let gz = GzDecoder::new(bytes);
        let mut archive = Archive::new(gz);
        archive.unpack(dest_dir)?;
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
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries {
            let entry = entry.ok().ok_or_else(|| PackageError::InvalidSpec("Failed to read directory".to_string()))?;
            let path = entry.path();
            if path.is_file() {
                let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                if stem.eq_ignore_ascii_case(expected_name) {
                    return Ok(path);
                }
            }
        }
    }
    Err(PackageError::InvalidSpec(format!("No executable found in '{}'", dir.display())))
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

fn format_bytes(bytes: usize) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut size = bytes as f64;
    let mut unit_idx = 0;

    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }

    if unit_idx == 0 {
        format!("{} {}", bytes, UNITS[unit_idx])
    } else {
        format!("{:.2} {}", size, UNITS[unit_idx])
    }
}
