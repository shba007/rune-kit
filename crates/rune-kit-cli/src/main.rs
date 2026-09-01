// crates/rune-kit-cli/src/main.rs
mod args;

use args::{Cli, Commands};
use clap::Parser;
use rune_kit_core::{McpRouter, PackageManager, WasmPluginInstance};
use std::collections::HashMap;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let pm = PackageManager::new();

    match cli.command {
        Commands::Install { target, version } => {
            let installed = pm.install(&target, version).await?;
            println!("Installed '{}' (v{})", installed.name, installed.version);
        }
        Commands::Uninstall { name } => {
            if pm.uninstall(&name)? {
                println!("Uninstalled '{}'", name);
            } else {
                eprintln!("Plugin '{}' is not installed", name);
            }
        }
        Commands::List => {
            let lockfile = pm.load_lockfile();
            if lockfile.plugins.is_empty() {
                println!(
                    "No plugins installed. Use `rune install <name>` or `rune install <path.wasm>`"
                );
                return Ok(());
            }
            println!(
                "{:<14} {:<10} {:<36} {}",
                "NAME", "VERSION", "DESCRIPTION", "SOURCE"
            );
            println!("{}", "-".repeat(78));
            for (name, p) in &lockfile.plugins {
                let desc = p.description.as_deref().unwrap_or("-");
                let short_desc = truncate_display(desc, 34);
                println!(
                    "{:<14} {:<10} {:<36} {}",
                    name, p.version, short_desc, p.source
                );
            }
        }
        Commands::Run { plugin, all, param } => {
            let mut router = McpRouter::new();
            let mut params_map = resolve_host_env_params();
            params_map.extend(param.into_iter());

            if all {
                let lockfile = pm.load_lockfile();
                if lockfile.plugins.is_empty() {
                    eprintln!("No plugins installed to run in gateway mode. Install one first.");
                    std::process::exit(1);
                }
                for (name, _) in lockfile.plugins {
                    if let Some(path) = pm.get_plugin_path(&name) {
                        let instance =
                            WasmPluginInstance::load_from_file(&name, path, params_map.clone())?;
                        router.register(name, instance);
                    }
                }
            } else if let Some(target) = plugin {
                let path = pm
                    .get_plugin_path(&target)
                    .unwrap_or_else(|| std::path::PathBuf::from(&target));
                let instance = WasmPluginInstance::load_from_file(&target, path, params_map)?;
                router.register(target, instance);
            } else {
                eprintln!("Error: Provide a plugin name/path or use `--all`");
                std::process::exit(1);
            }

            router.run_stdio()?;
        }
        Commands::Update { name } => {
            eprintln!(
                "rune update is not implemented yet (requested: {}). \
                 Reinstall with `rune install <target>` to fetch the latest version for now.",
                name.as_deref().unwrap_or("all plugins")
            );
            std::process::exit(1);
        }
    }

    Ok(())
}

/// Truncates on a char boundary (never a byte boundary) so descriptions
/// containing multi-byte UTF-8 characters can't panic the `list` command.
fn truncate_display(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut truncated: String = s.chars().take(max_chars.saturating_sub(3)).collect();
    truncated.push_str("...");
    truncated
}

/// Dynamically injects all host environment variables into Extism config as lowercase keys.
fn resolve_host_env_params() -> HashMap<String, String> {
    let mut map = HashMap::new();
    for (k, v) in std::env::vars() {
        map.insert(k.to_ascii_lowercase(), v);
    }
    map
}
