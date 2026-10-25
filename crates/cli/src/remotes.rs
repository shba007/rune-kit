use clap::Subcommand;
use rune_kit_core::remote::RemoteConfig;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

#[derive(Subcommand)]
pub enum Action {
    /// Register an external Streamable HTTP MCP server (restart serving to apply)
    Add {
        name: String,
        endpoint: String,
        #[arg(long, default_value = "2026-07-28")]
        protocol: String,
        /// Environment variable containing the upstream bearer token
        #[arg(long)]
        token_env: Option<String>,
    },
    List,
    Remove {
        name: String,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    schema_version: u32,
    servers: BTreeMap<String, RemoteConfig>,
}
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.contains("__")
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}
pub fn read(path: &Path) -> Result<BTreeMap<String, RemoteConfig>, Box<dyn std::error::Error>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(e.into()),
    };
    let doc: Document = serde_json::from_slice(&bytes)
        .map_err(|_| "Cannot parse remotes.json; repair it before continuing")?;
    if doc.schema_version != 1 {
        return Err("Unsupported remotes.json schema".into());
    }
    for (name, config) in &doc.servers {
        if !valid_name(name) {
            return Err("Invalid remote server namespace".into());
        }
        config.validate()?;
    }
    Ok(doc.servers)
}
struct Lock(std::path::PathBuf);
impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
pub fn run(
    action: Action,
    root: &Path,
    json_output: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = root.join("remotes.json");
    if matches!(action, Action::List) {
        let servers = read(&path)?;
        let values: Vec<_> = servers.iter().map(|(name,c)| serde_json::json!({"name":name,"endpoint":c.redacted_endpoint(),"protocol":c.protocol,"token_env":c.token_env})).collect();
        if json_output {
            println!("{}", serde_json::to_string(&values)?);
        } else {
            for value in values {
                println!(
                    "{} {} ({})",
                    value["name"].as_str().unwrap(),
                    value["endpoint"].as_str().unwrap(),
                    value["protocol"].as_str().unwrap()
                );
            }
        }
        return Ok(());
    }
    std::fs::create_dir_all(root)?;
    let lock_path = root.join("remotes.lock");
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
        .map_err(|_| "Another remote configuration save is active")?;
    let _guard = Lock(lock_path);
    drop(lock);
    let mut servers = read(&path)?;
    match action {
        Action::Add {
            name,
            endpoint,
            protocol,
            token_env,
        } => {
            if !valid_name(&name) {
                return Err("Use a lowercase namespace of at most 64 letters, digits, hyphens or underscores; double underscores are reserved".into());
            }
            if servers.contains_key(&name) {
                return Err(
                    "Remote already exists; remove it explicitly before replacing it".into(),
                );
            }
            let installed = root.join("installed.json");
            if installed.exists() {
                let installed: rune_kit_core::Lockfile =
                    serde_json::from_slice(&std::fs::read(installed)?)?;
                if installed.plugins.contains_key(&name) {
                    return Err("Namespace already belongs to an installed plugin".into());
                }
            }
            let config = RemoteConfig {
                endpoint,
                protocol,
                token_env,
            };
            config.validate()?;
            servers.insert(name, config);
        }
        Action::Remove { name } => {
            if servers.remove(&name).is_none() {
                return Err("Remote server does not exist".into());
            }
        }
        Action::List => unreachable!(),
    }
    write(root, servers)?;
    if json_output {
        println!("{{\"saved\":true}}");
    } else {
        println!("Remote configuration saved. Restart serving to apply it.");
    }
    Ok(())
}

fn write(
    root: &Path,
    servers: BTreeMap<String, RemoteConfig>,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let path = root.join("remotes.json");
    let bytes = serde_json::to_vec_pretty(&Document {
        schema_version: 1,
        servers,
    })?;
    let stage = root.join(format!("remotes.{}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&stage)?;
    let result = (|| -> std::io::Result<()> {
        use std::io::Write;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&stage, &path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(stage);
    }
    result?;
    Ok(bytes)
}

pub(crate) fn save_dashboard(
    root: &Path,
    expected: &Option<Vec<u8>>,
    servers: BTreeMap<String, RemoteConfig>,
) -> Result<Vec<u8>, String> {
    std::fs::create_dir_all(root).map_err(|_| "Cannot create remote configuration storage")?;
    let lock_path = root.join("remotes.lock");
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
        .map_err(|_| "Another remote configuration save is active")?;
    let _guard = Lock(lock_path);
    drop(file);
    let current = bytes(root)?;
    if &current != expected {
        return Err(
            "Remote configuration changed outside this panel; reopen it before editing".into(),
        );
    }
    for (name, config) in &servers {
        if !valid_name(name) {
            return Err("Invalid namespace: use lowercase letters, digits, hyphens or underscores; no double underscores".into());
        }
        config.validate().map_err(|e| e.to_string())?;
    }
    let installed = root.join("installed.json");
    if installed.exists() {
        let installed: rune_kit_core::Lockfile = serde_json::from_slice(
            &std::fs::read(installed).map_err(|_| "Cannot read installed plugins")?,
        )
        .map_err(|_| "Cannot parse installed plugins")?;
        if servers
            .keys()
            .any(|name| installed.plugins.contains_key(name))
        {
            return Err("Namespace already belongs to an installed plugin".into());
        }
    }
    write(root, servers)
        .map_err(|_| "Cannot save remote configuration; previous settings retained".into())
}
pub(crate) fn bytes(root: &Path) -> Result<Option<Vec<u8>>, String> {
    match std::fs::read(root.join("remotes.json")) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err("Cannot read remote configuration".into()),
    }
}
