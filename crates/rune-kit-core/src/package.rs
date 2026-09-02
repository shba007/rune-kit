use crate::manifest::{ExecutionKind, InstalledPlugin, Lockfile};
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
                .last()
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

        // 1. Probe metadata (prefer WASM probe; fallback to native probe)
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

        if let Some(ref wasm_bytes) = bundle.wasm_bytes {
            if let Ok(mut instance) = WasmPluginInstance::load_from_bytes(
                &bundle.name,
                wasm_bytes.clone(),
                probe_params.clone(),
            ) {
                if let Ok(info) = instance.get_info() {
                    final_name = if bundle.name.is_empty() {
                        info.name
                    } else {
                        bundle.name.clone()
                    };
                    final_ver = bundle.version.clone().unwrap_or(info.version);
                    final_desc = info.description;
                }
            }
        }

        sanitize_path_component(&final_name, "plugin name")?;
        sanitize_path_component(&final_ver, "plugin version")?;

        // 2. Save WASM binary if downloaded
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

        // 3. Extract & save Native binary if downloaded
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

            // If metadata probe hadn't run via wasm, probe via native binary
            if final_desc.is_none() {
                if let Ok(mut sidecar) =
                    NativeSidecar::new(&final_name, &extracted_binary, probe_params)
                {
                    if let Ok(info) = sidecar.get_info() {
                        final_ver = bundle.version.clone().unwrap_or(info.version);
                        final_desc = info.description;
                    }
                }
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
                        n.get(&format!(
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

pub fn current_target_triple() -> &'static str {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        "x86_64-pc-windows-msvc"
    }
    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    {
        "aarch64-pc-windows-msvc"
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        "x86_64-unknown-linux-gnu"
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        "aarch64-unknown-linux-gnu"
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        "x86_64-apple-darwin"
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        "aarch64-apple-darwin"
    }
    #[cfg(not(any(
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "aarch64"),
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64"),
    )))]
    {
        "unknown"
    }
}

fn is_native_archive_or_binary(target: &str) -> bool {
    target.ends_with(".zip")
        || target.ends_with(".tar.gz")
        || target.ends_with(".tgz")
        || (cfg!(windows) && target.ends_with(".exe"))
        || (!target.ends_with(".wasm") && Path::new(target).is_file())
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
    let mut candidates = Vec::new();

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
                let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");

                if stem.eq_ignore_ascii_case(expected_name)
                    || stem.eq_ignore_ascii_case(&format!("{}-native", expected_name))
                    || stem.eq_ignore_ascii_case(&expected_name.replace('-', "_"))
                    || stem.eq_ignore_ascii_case(&format!(
                        "{}_native",
                        expected_name.replace('-', "_")
                    ))
                {
                    return Ok(path);
                }

                if cfg!(windows) {
                    if path
                        .extension()
                        .map_or(false, |ext| ext.eq_ignore_ascii_case("exe"))
                    {
                        candidates.push(path);
                    }
                } else if !file_name.ends_with(".txt")
                    && !file_name.ends_with(".md")
                    && !file_name.ends_with(".json")
                {
                    candidates.push(path);
                }
            } else if path.is_dir() {
                if let Ok(found) = find_executable_in_dir(&path, expected_name) {
                    return Ok(found);
                }
            }
        }
    }

    if let Some(first) = candidates.first() {
        return Ok(first.clone());
    }

    Err(PackageError::InvalidSpec(format!(
        "No executable binary found in unpacked archive for '{}'",
        expected_name
    )))
}

fn set_executable_permissions(path: &Path) -> Result<(), std::io::Error> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = std::fs::metadata(path) {
            let mut perms = metadata.permissions();
            perms.set_mode(perms.mode() | 0o755);
            std::fs::set_permissions(path, perms)?;
        }
    }
    let _ = path;
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

fn sanitize_path_component(value: &str, label: &str) -> Result<(), PackageError> {
    if value.is_empty() || value.contains(['/', '\\']) || value.contains("..") {
        return Err(PackageError::InvalidSpec(format!(
            "Invalid {}: '{}' must not contain path separators",
            label, value
        )));
    }
    Ok(())
}
