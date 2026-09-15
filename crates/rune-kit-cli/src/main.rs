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
            render_list_table(&lockfile).render();
        }
        Commands::Available => {
            let lockfile = pm.load_lockfile();
            let available = pm.fetch_registry().await?;
            if available.is_empty() {
                println!("No plugins found in the registry.");
                return Ok(());
            }
            render_registry_table(&available, &lockfile).render();
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
            render_registry_table(&matches, &lockfile).render();
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
                render_update_status_table(&statuses).render();
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

fn render_registry_table(plugins: &[RegistryPluginSummary], lockfile: &Lockfile) -> Table {
    let mut t = Table::new(88)
        .column(Column::fixed("NAME", 20))
        .column(Column::fixed("CURRENT", 10))
        .column(Column::fixed("LATEST", 10))
        .column(Column::fixed("BUILDS", 14))
        .column(Column::flexible("DESCRIPTION", 30));
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
        t.add_row(vec![
            p.name.clone(),
            current.to_string(),
            p.latest.clone(),
            builds.to_string(),
            p.description.clone().unwrap_or_else(|| "-".to_string()),
        ]);
    }
    t
}

fn render_update_status_table(statuses: &[PluginUpdateStatus]) -> Table {
    let mut t = Table::new(70)
        .column(Column::fixed("PLUGIN", 16))
        .column(Column::fixed("CURRENT", 12))
        .column(Column::fixed("LATEST", 12))
        .column(Column::fixed("STATUS", 14))
        .column(Column::flexible("BUILDS", 12));
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
        t.add_row(vec![
            s.name.clone(),
            s.installed_version.clone(),
            s.latest_version.clone(),
            status.to_string(),
            builds.to_string(),
        ]);
    }
    t
}

fn resolve_host_env_params() -> HashMap<String, String> {
    let mut map = HashMap::new();
    for (k, v) in std::env::vars() {
        map.insert(k.to_ascii_lowercase(), v);
    }
    map
}

fn render_list_table(lockfile: &Lockfile) -> Table {
    let mut t = Table::new(84)
        .column(Column::fixed("NAME", 14))
        .column(Column::fixed("VERSION", 10))
        .column(Column::fixed("BUILDS", 8))
        .column(Column::fixed("DESCRIPTION", 34))
        .column(Column::flexible("SOURCE", 14));
    for (name, p) in &lockfile.plugins {
        let builds = match (p.binary_path.ends_with(".wasm"), p.native_binary_path.is_some()) {
            (true, true) => "wasm, native",
            (true, false) => "wasm",
            (false, true) => "native",
            (false, false) => "-",
        };
        let desc = p.description.clone().unwrap_or_else(|| "-".to_string());
        let short_desc = truncate_display(&desc, 32);
        t.add_row(vec![
            name.to_string(),
            p.version.clone(),
            builds.to_string(),
            short_desc,
            p.source.clone(),
        ]);
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn sample_lockfile() -> Lockfile {
        let mut plugins = HashMap::new();
        plugins.insert(
            "alpha".to_string(),
            InstalledPlugin {
                name: "alpha".to_string(),
                description: Some("A sample plugin".to_string()),
                version: "1.2.3".to_string(),
                binary_path: "alpha.wasm".to_string(),
                native_binary_path: Some("alpha-bin".to_string()),
                execution_kind: ExecutionKind::Wasm,
                sha256: "abc".to_string(),
                source: "file:///alpha.wasm".to_string(),
                default_params: HashMap::new(),
            },
        );
        Lockfile { version: 1, plugins }
    }

    #[test]
    fn registry_table_header_matches_layout() {
        let rows = render_registry_table(&[], &sample_lockfile()).format_rows();
        assert_eq!(
            rows[0],
            format!("{:<20} {:<10} {:<10} {:<14} {:<30}", "NAME", "CURRENT", "LATEST", "BUILDS", "DESCRIPTION")
        );
    }

    #[test]
    fn update_table_header_matches_layout() {
        let rows = render_update_status_table(&[]).format_rows();
        assert_eq!(
            rows[0],
            format!("{:<16} {:<12} {:<12} {:<14} {:<12}", "PLUGIN", "CURRENT", "LATEST", "STATUS", "BUILDS")
        );
    }

    #[test]
    fn list_table_header_matches_layout() {
        let rows = render_list_table(&sample_lockfile()).format_rows();
        assert_eq!(
            rows[0],
            format!("{:<14} {:<10} {:<8} {:<34} {:<14}", "NAME", "VERSION", "BUILDS", "DESCRIPTION", "SOURCE")
        );
    }
}