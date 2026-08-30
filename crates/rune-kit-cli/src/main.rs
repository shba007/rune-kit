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
                "{:<16} {:<10} {:<12} {}",
                "NAME", "VERSION", "SOURCE", "SHA256"
            );
            println!("{}", "-".repeat(56));
            for (name, p) in &lockfile.plugins {
                let short_sha = if p.sha256.len() >= 8 {
                    &p.sha256[..8]
                } else {
                    &p.sha256
                };
                println!(
                    "{:<16} {:<10} {:<12} {}...",
                    name, p.version, p.source, short_sha
                );
            }
        }
        Commands::Run { plugin, all, param } => {
            let mut router = McpRouter::new();
            let params_map: HashMap<String, String> = param.into_iter().collect();

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
            println!(
                "Checking updates for {:?}...",
                name.as_deref().unwrap_or("all plugins")
            );
        }
    }

    Ok(())
}
