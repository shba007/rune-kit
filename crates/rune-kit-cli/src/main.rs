mod args;

mod output;

use args::{Cli, Commands};
use clap::Parser;
use output::{Column, Table};
use rune_kit_core::{
    ExecutionKind, Lockfile, McpRouter, PackageManager, PluginInstance, PluginUpdateStatus,
    RegistryPluginSummary, is_newer_version,
};
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
                "{:<14} {:<10} {:<12} {:<34} {}",
                "NAME", "VERSION", "BUILDS", "DESCRIPTION", "SOURCE"
            );
            println!("{}", "-".repeat(84));
            for (name, p) in &lockfile.plugins {
                let desc = p.description.as_deref().unwrap_or("-");
                let short_desc = truncate_display(desc, 32);
                let builds = match (p.binary_path.ends_with(".wasm"), p.native_binary_path.is_some()) {
                    (true, true) => "wasm, native",
                    (true, false) => "wasm",
                    (false, true) => "native",
                    (false, false) => "-",
                };
                println!(
                    "{:<14} {:<10} {:<8} {:<34} {}",
                    name, p.version, builds, short_desc, p.source
                );
            }
        }
        Commands::Available => {
            let lockfile = pm.load_lockfile();
            let available = pm.fetch_registry().await?;
            if available.is_empty() {
                println!("No plugins found in the registry.");
                return Ok(());
            }
            render_registry_table(&available, &lockfile);
        }
        Commands::Search { keyword } => {
            let lockfile = pm.load_lockfile();
            let available = pm.fetch_registry().await?;
            let kw_lower = keyword.to_lowercase();
            let matches: Vec<_> = available
                .into_iter()
                .filter(|p| {
                    p.name.to_lowercase().contains(&kw_lower)
                        || p.description
                            .as_deref()
                            .unwrap_or("")
                            .to_lowercase()
                            .contains(&kw_lower)
                })
                .collect();

            if matches.is_empty() {
                println!("No plugins found matching '{}'", keyword);
                return Ok(());
            }
            render_registry_table(&matches, &lockfile);
        }
        Commands::Update {
            names,
            check,
            native,
        } => {
            let filter = if names.is_empty() {
                None
            } else {
                Some(names.as_slice())
            };
            let statuses = pm.check_plugin_updates(filter).await?;

            if statuses.is_empty() {
                println!("No matching installed plugins found to inspect or update.");
                return Ok(());
            }

            if check {
                render_update_status_table(&statuses);
                return Ok(());
            }

            let to_update: Vec<_> = statuses.into_iter().filter(|s| s.has_update).collect();
            if to_update.is_empty() {
                println!("All specified plugins are already up to date.");
                return Ok(());
            }

            for item in to_update {
                println!(
                    "Updating '{}' (v{} -> v{})...",
                    item.name, item.installed_version, item.latest_version
                );
                let installed = pm
                    .install(&item.name, Some(item.latest_version), native)
                    .await?;
                println!(
                    "Successfully updated '{}' to v{}",
                    installed.name, installed.version
                );
            }
        }
        Commands::SelfUpdate {
            check,
            version,
            force,
        } => {
            let current_ver = env!("CARGO_PKG_VERSION");
            if check {
                let (tag, _) = pm.fetch_latest_cli_release(version.as_deref()).await?;
                let latest_clean = tag.strip_prefix('v').unwrap_or(&tag);
                println!("Current version: v{}", current_ver);
                println!("Release target:  {}", tag);
                if is_newer_version(latest_clean, current_ver) {
                    println!("\nUpdate available! Run `rune self-update` to install.");
                } else if latest_clean == current_ver {
                    println!("\nYou are already on the latest version.");
                } else {
                    println!("\nCurrent version is newer than the targeted release.");
                }
                return Ok(());
            }

            println!("Checking for CLI release...");
            match pm.self_update(version.as_deref(), force).await {
                Ok(new_tag) => {
                    println!("Successfully updated rune CLI to {}!", new_tag);
                }
                Err(rune_kit_core::PackageError::SelfUpdate(msg)) => {
                    eprintln!("{}", msg);
                }
                Err(err) => {
                    eprintln!("Failed to perform self-update: {}", err);
                }
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

fn render_registry_table(plugins: &[RegistryPluginSummary], lockfile: &Lockfile) {
    println!(
        "{:<20} {:<10} {:<10} {:<14} {}",
        "NAME", "CURRENT", "LATEST", "BUILDS", "DESCRIPTION"
    );
    println!("{}", "-".repeat(88));
    for p in plugins {
        let builds = match (p.has_wasm, p.has_native) {
            (true, true) => "wasm, native",
            (true, false) => "wasm",
            (false, true) => "native",
            (false, false) => "-",
        };
        let current = lockfile
            .plugins
            .get(&p.name)
            .map(|e| e.version.as_str())
            .unwrap_or("-");
        let desc = p.description.as_deref().unwrap_or("-");
        let short_desc = truncate_display(desc, 40);
        println!(
            "{:<20} {:<10} {:<10} {:<14} {}",
            p.name, current, p.latest, builds, short_desc
        );
    }
}

fn render_update_status_table(statuses: &[PluginUpdateStatus]) {
    println!(
        "{:<16} {:<12} {:<12} {:<14} {}",
        "PLUGIN", "CURRENT", "LATEST", "STATUS", "BUILDS"
    );
    println!("{}", "-".repeat(70));
    for s in statuses {
        let status = if s.has_update {
            "Updateable"
        } else {
            "Up-to-date"
        };
        let builds = match (s.has_wasm, s.has_native) {
            (true, true) => "wasm, native",
            (true, false) => "wasm",
            (false, true) => "native",
            (false, false) => "-",
        };
        println!(
            "{:<16} {:<12} {:<12} {:<14} {}",
            s.name, s.installed_version, s.latest_version, status, builds
        );
    }
}

fn resolve_host_env_params() -> HashMap<String, String> {
    let mut map = HashMap::new();
    for (k, v) in std::env::vars() {
        map.insert(k.to_ascii_lowercase(), v);
    }
    map
}