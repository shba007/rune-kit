mod args;

use args::{Cli, Commands};
use clap::Parser;
use rune_kit_core::{ExecutionKind, McpRouter, PackageManager, PluginInstance};
use std::collections::HashMap;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let pm = PackageManager::new();

    match cli.command {
        Commands::Install {
            target,
            version,
            native,
        } => {
            let installed = pm.install(&target, version, native).await?;
            let kind_label = match (&installed.native_binary_path, &installed.execution_kind) {
                (Some(_), ExecutionKind::Native) => "native",
                (Some(_), ExecutionKind::Wasm) => "wasm+native",
                (None, ExecutionKind::Native) => "native",
                (None, ExecutionKind::Wasm) => "wasm",
            };
            println!(
                "Installed '{}' (v{}, {})",
                installed.name, installed.version, kind_label
            );
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
                "{:<14} {:<10} {:<8} {:<34} {}",
                "NAME", "VERSION", "KIND", "DESCRIPTION", "SOURCE"
            );
            println!("{}", "-".repeat(84));
            for (name, p) in &lockfile.plugins {
                let desc = p.description.as_deref().unwrap_or("-");
                let short_desc = truncate_display(desc, 32);
                let kind_label = match p.execution_kind {
                    ExecutionKind::Wasm => "wasm",
                    ExecutionKind::Native => "native",
                };
                println!(
                    "{:<14} {:<10} {:<8} {:<34} {}",
                    name, p.version, kind_label, short_desc, p.source
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
                for (name, plugin_entry) in lockfile.plugins {
                    if let Some(path) = pm.get_plugin_path(&name) {
                        let instance = match plugin_entry.execution_kind {
                            ExecutionKind::Wasm => PluginInstance::load_wasm_from_file(
                                &name,
                                path,
                                params_map.clone(),
                            )?,
                            ExecutionKind::Native => PluginInstance::load_native_from_file(
                                &name,
                                path,
                                params_map.clone(),
                            )?,
                        };
                        router.register(name, instance);
                    }
                }
            } else if let Some(target) = plugin {
                let lockfile = pm.load_lockfile();
                let (path, execution_kind) = if let Some(entry) = lockfile.plugins.get(&target) {
                    (
                        pm.get_plugin_path(&target).unwrap(),
                        entry.execution_kind.clone(),
                    )
                } else {
                    let p = std::path::PathBuf::from(&target);
                    let kind = if p.extension().map_or(false, |ext| ext == "wasm") {
                        ExecutionKind::Wasm
                    } else {
                        ExecutionKind::Native
                    };
                    (p, kind)
                };

                let instance = match execution_kind {
                    ExecutionKind::Wasm => {
                        PluginInstance::load_wasm_from_file(&target, path, params_map)?
                    }
                    ExecutionKind::Native => {
                        PluginInstance::load_native_from_file(&target, path, params_map)?
                    }
                };
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

fn truncate_display(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut truncated: String = s.chars().take(max_chars.saturating_sub(3)).collect();
    truncated.push_str("...");
    truncated
}

fn resolve_host_env_params() -> HashMap<String, String> {
    let mut map = HashMap::new();
    for (k, v) in std::env::vars() {
        map.insert(k.to_ascii_lowercase(), v);
    }
    map
}
