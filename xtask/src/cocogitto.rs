//! Process boundary isolates Cocogitto's logging dependencies from Wasmtime.
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
#[allow(dead_code)]
pub const GENERATOR: &str = "cocogitto@7.0.0";
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Signature {
    pub name: Option<String>,
    pub timestamp: i64,
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Commit {
    pub id: String,
    pub message: String,
    pub author: Signature,
}
impl Commit {
    pub fn new(id: String, message: String) -> Self {
        Self {
            id,
            message,
            author: Signature::default(),
        }
    }
}
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct Release {
    pub version: Option<String>,
    pub commits: Vec<Commit>,
}
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct Generated {
    pub releases: Vec<Release>,
    pub markdown: String,
}
fn invoke(request: serde_json::Value) -> Result<serde_json::Value> {
    static BINARY: OnceLock<PathBuf> = OnceLock::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("generator");
    {
        static BUILD: Mutex<()> = Mutex::new(());
        let _guard = BUILD.lock().map_err(|_| "Generator build lock poisoned")?;
        if BINARY.get().is_none() {
            let target = root.join("target");
            let status = Command::new("cargo")
                .args(["build", "--locked", "--quiet", "--manifest-path"])
                .arg(root.join("Cargo.toml"))
                .arg("--target-dir")
                .arg(&target)
                .env("CARGO_HUSKY_DONT_INSTALL_HOOKS", "true")
                .status()?;
            if !status.success() {
                return Err("Cocogitto generator build failed".into());
            }
            let _ = BINARY.set(target.join("debug").join(format!(
                "rune-cocogitto-generator{}",
                std::env::consts::EXE_SUFFIX
            )));
        }
    }
    let mut child = Command::new(BINARY.get().unwrap())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&request)?)?;
    let result = child.wait_with_output()?;
    if !result.status.success() {
        return Err(String::from_utf8_lossy(&result.stderr).into_owned().into());
    }
    Ok(serde_json::from_slice(&result.stdout)?)
}
pub fn generate(
    base: &str,
    commits: Vec<Commit>,
    timestamp: i64,
    _package: Option<&str>,
) -> Result<Generated> {
    Ok(serde_json::from_value(invoke(
        serde_json::json!({"operation":"generate", "base":base,"commits":commits,"timestamp":timestamp}),
    )?)?)
}

#[allow(dead_code)]
pub fn render_at(
    base: Option<&str>,
    version: &str,
    commits: &[Commit],
    timestamp: i64,
    _package: Option<&str>,
) -> Result<String> {
    Ok(serde_json::from_value(invoke(
        serde_json::json!({"operation":"render", "base":base,"version":version,"commits":commits,"timestamp":timestamp}),
    )?)?)
}
