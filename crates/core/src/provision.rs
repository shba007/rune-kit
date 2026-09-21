use crate::manifest::BinaryDependencySpec;
use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use tar::Archive;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ProvisionError {
    #[error("IO error during binary provisioning: {0}")]
    Io(#[from] std::io::Error),
    #[error("Network download error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("Checksum mismatch for binary '{name}': expected {expected}, actual {actual}")]
    ChecksumMismatch {
        name: String,
        expected: String,
        actual: String,
    },
    #[error("Binary '{0}' is not provisioned and no download source was found")]
    NotFound(String),
    #[error("Binary '{0}' is not in capabilities.exec.allowed_binaries allowlist")]
    Unauthorized(String),
    #[error("{name} is currently being provisioned ({progress_percent}% downloaded). Please retry in {eta_seconds} seconds.")]
    InProgress {
        name: String,
        progress_percent: u8,
        eta_seconds: u64,
    },
    #[error("Archive extraction failed: {0}")]
    Extraction(String),
    #[error("Failed to acquire process lock: {0}")]
    LockError(String),
}

pub struct FileLock {
    lock_path: PathBuf,
}

impl FileLock {
    pub fn acquire(lock_path: PathBuf) -> Result<Self, ProvisionError> {
        if lock_path.exists() {
            if let Ok(metadata) = std::fs::metadata(&lock_path)
                && let Ok(modified) = metadata.modified()
                && let Ok(elapsed) = modified.elapsed()
                && elapsed.as_secs() > 600
            {
                let _ = std::fs::remove_file(&lock_path);
            } else {
                return Err(ProvisionError::InProgress {
                    name: lock_path
                        .parent()
                        .and_then(|p| p.file_name())
                        .and_then(|s| s.to_str())
                        .unwrap_or("binary")
                        .to_string(),
                    progress_percent: 50,
                    eta_seconds: 30,
                });
            }
        }

        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
            .map_err(|e| ProvisionError::LockError(e.to_string()))?;

        Ok(Self { lock_path })
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.lock_path);
    }
}

#[derive(Clone, Debug)]
pub struct BinaryProvisioner {
    base_dir: PathBuf,
}

impl Default for BinaryProvisioner {
    fn default() -> Self {
        Self::new()
    }
}

impl BinaryProvisioner {
    pub fn new() -> Self {
        let base_dir = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("rune-kit")
            .join("_bin");
        let _ = std::fs::create_dir_all(&base_dir);
        Self { base_dir }
    }

    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    pub fn resolve_binary(
        &self,
        name: &str,
        allowed_binaries: &[String],
    ) -> Result<PathBuf, ProvisionError> {
        if !allowed_binaries.iter().any(|b| b == name) {
            return Err(ProvisionError::Unauthorized(name.to_string()));
        }

        let bin_dir = self.base_dir.join(name);
        let lock_file = bin_dir.join(".lock");
        if lock_file.exists() {
            return Err(ProvisionError::InProgress {
                name: name.to_string(),
                progress_percent: 45,
                eta_seconds: 30,
            });
        }

        if bin_dir.exists()
            && let Ok(entries) = std::fs::read_dir(&bin_dir)
        {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let verified_marker = path.join(".verified");
                    if verified_marker.exists() {
                        let candidate = path.join(name);
                        let candidate_exe = path.join(format!("{}.exe", name));
                        if candidate.exists() {
                            return Ok(candidate);
                        }
                        if candidate_exe.exists() {
                            return Ok(candidate_exe);
                        }
                    }
                }
            }
        }

        Err(ProvisionError::NotFound(name.to_string()))
    }

    pub async fn provision_binary(
        &self,
        name: &str,
        spec: &BinaryDependencySpec,
    ) -> Result<PathBuf, ProvisionError> {
        let target_version = if spec.version.is_empty() {
            "latest"
        } else {
            spec.version.trim_start_matches(">=").trim()
        };

        let target_dir = self.base_dir.join(name).join(target_version);
        let verified_marker = target_dir.join(".verified");

        let candidate = target_dir.join(name);
        let candidate_exe = target_dir.join(format!("{}.exe", name));
        if verified_marker.exists() {
            if candidate.exists() {
                return Ok(candidate);
            }
            if candidate_exe.exists() {
                return Ok(candidate_exe);
            }
        }

        let lock_path = self.base_dir.join(name).join(".lock");
        let _guard = FileLock::acquire(lock_path)?;

        let download_url = spec.url.clone().ok_or_else(|| {
            ProvisionError::NotFound(format!(
                "No download URL configured for required dependency '{}'",
                name
            ))
        })?;

        let client = reqwest::Client::builder()
            .user_agent("rune-kit-provisioner")
            .build()?;
        let bytes = client.get(&download_url).send().await?.bytes().await?.to_vec();

        if let Some(ref expected_sha) = spec.sha256 {
            let actual_sha: String = Sha256::digest(&bytes)
                .iter()
                .map(|b| format!("{:02x}", b))
                .collect();
            if !actual_sha.eq_ignore_ascii_case(expected_sha) {
                return Err(ProvisionError::ChecksumMismatch {
                    name: name.to_string(),
                    expected: expected_sha.clone(),
                    actual: actual_sha,
                });
            }
        }

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let temp_dir = self
            .base_dir
            .join(name)
            .join(format!(".partial-{}-{}", target_version, timestamp));
        std::fs::create_dir_all(&temp_dir)?;

        extract_pure_rust(&bytes, &temp_dir)?;

        let exe_path = find_executable(&temp_dir, name)?;
        set_executable_permissions(&exe_path)?;

        std::fs::write(temp_dir.join(".verified"), b"verified")?;

        if target_dir.exists() {
            let _ = std::fs::remove_dir_all(&target_dir);
        }

        if std::fs::rename(&temp_dir, &target_dir).is_err() {
            copy_dir_recursive(&temp_dir, &target_dir)?;
            let _ = std::fs::remove_dir_all(&temp_dir);
        }

        let final_bin = if target_dir.join(name).exists() {
            target_dir.join(name)
        } else {
            target_dir.join(format!("{}.exe", name))
        };

        set_executable_permissions(&final_bin)?;
        Ok(final_bin)
    }
}

fn extract_pure_rust(bytes: &[u8], dest_dir: &Path) -> Result<(), ProvisionError> {
    if bytes.starts_with(b"PK\x03\x04") {
        let reader = Cursor::new(bytes);
        let mut zip = zip::ZipArchive::new(reader)
            .map_err(|e| ProvisionError::Extraction(format!("ZIP error: {}", e)))?;
        for i in 0..zip.len() {
            let mut file = zip
                .by_index(i)
                .map_err(|e| ProvisionError::Extraction(format!("ZIP file error: {}", e)))?;
            let enclosed = file
                .enclosed_name()
                .ok_or_else(|| ProvisionError::Extraction("ZIP traversal detected".to_string()))?;
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
        Ok(())
    } else if bytes.starts_with(b"\x1f\x8b") {
        let gz = GzDecoder::new(bytes);
        let mut archive = Archive::new(gz);
        archive
            .unpack(dest_dir)
            .map_err(|e| ProvisionError::Extraction(format!("Tar-GZ error: {}", e)))?;
        Ok(())
    } else {
        let default_name = "binary";
        let outpath = dest_dir.join(default_name);
        std::fs::write(&outpath, bytes)?;
        Ok(())
    }
}

fn find_executable(dir: &Path, expected_name: &str) -> Result<PathBuf, ProvisionError> {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                if stem.eq_ignore_ascii_case(expected_name) {
                    return Ok(path);
                }
            } else if path.is_dir()
                && let Ok(sub_exe) = find_executable(&path, expected_name)
            {
                return Ok(sub_exe);
            }
        }
    }
    Err(ProvisionError::NotFound(format!(
        "Executable '{}' not found in extracted archive",
        expected_name
    )))
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

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let target = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}