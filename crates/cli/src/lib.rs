use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "rune", version, about = "Rune MCP Engine & Package Manager")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Start the MCP server gateway over stdio
    Run {
        /// Run a specific plugin by name or local file path
        plugin: Option<String>,
        /// Run all installed plugins simultaneously in gateway mode
        #[arg(long)]
        all: bool,
        /// Injected key-value parameter (e.g. -p allowed_dir=/workspace)
        #[arg(short, long, value_parser = parse_key_val)]
        param: Vec<(String, String)>,
    },
    /// Install an artifact from registry, URL, or local path
    ///
    /// For plugins (MCP tools): uses WASM or native sidecar execution model
    /// For skills (content): standalone documentation artifacts that guide tool use
    Install {
        target: String,
        #[arg(short, long)]
        version: Option<String>,
        #[arg(long)]
        #[arg(hide = true)]
        _reserved_for_future: bool,
    },
    /// Uninstall an installed artifact (plugin or skill)
    Uninstall {
        #[arg(value_parser = parse_uninstall_name)]
        name: String,
    },
    /// List installed artifacts (plugins and skills)
    List,
    /// List available artifacts in the remote registry
    Available,
    /// Search artifacts in the registry by keyword across names and descriptions
    Search {
        /// Search keyword to match against artifact name or description
        keyword: String,
    },
    /// Update installed artifacts from the registry or inspect update availability
    #[command(alias = "outdated")]
    Update {
        /// Specific artifacts to update or inspect (updates all if omitted)
        names: Vec<String>,
        /// Check for available updates without downloading
        #[arg(short, long)]
        check: bool,
        #[arg(long)]
        #[arg(hide = true)]
        _reserved_for_future: bool,
    },
    /// Self-update the rune CLI binary to the latest release
    SelfUpdate {
        /// Check if an update is available without downloading
        #[arg(short, long)]
        check: bool,
        /// Specific release version to target (e.g. 0.1.4)
        #[arg(short, long)]
        version: Option<String>,
        /// Reinstall even if the binary is already up to date
        #[arg(short, long)]
        force: bool,
    },
}

/// A unified artifact from search results (plugin or skill)
#[derive(Debug, Clone)]
struct SearchResult {
    ty: String,
    name: String,
    latest: String,
    description: String,
}

fn parse_uninstall_name(s: &str) -> Result<String, String> {
    Ok(s.to_string())
}

fn parse_key_val(s: &str) -> Result<(String, String), String> {
    s.split_once('=')
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .ok_or_else(|| format!("Invalid KEY=VALUE pair: {}", s))
}

pub mod output;

use crate::output::{
    Column, Table, render_list_table, render_update_status_table, truncate_display,
};
use rune_kit_core::{
    ExecutionKind, McpRouter, PackageManager, PluginInstance, PluginUpdateStatus, SkillManager,
    is_newer_version,
};
use std::collections::HashMap;

pub async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let pm = PackageManager::new();
    let sm = SkillManager::new();

    match cli.command {
        Commands::Run { plugin, all, param } => {
            let mut router = McpRouter::new();
            let mut params_map = resolve_host_env_params();
            params_map.extend(param);

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
                    let kind = if p.extension().is_some_and(|ext| ext == "wasm") {
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
        Commands::Install {
            target,
            version,
            _reserved_for_future,
        } => {
            // Detect if target is a skill or plugin
            let is_skill = target.ends_with(".md")
                || target.ends_with(".txt")
                || target.ends_with(".rst")
                || target.ends_with(".markdown")
                || target.contains("__")
                || target.starts_with("#");

            if is_skill {
                // Install as skill
                let installed = sm.install_skill(&target, version).await?;
                println!(
                    "Installed skill '{}' (v{}, {}) — {}",
                    installed.name,
                    installed.version,
                    installed.source,
                    installed.description.unwrap_or_else(|| "-".to_string()),
                );
            } else {
                // Install as plugin
                let installed = pm.install(&target, version, false).await?;
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

                // Show additional info if available
                if let Some(ref desc) = installed.description
                    && !desc.is_empty()
                {
                    println!("  Description: {}", desc);
                }
                if installed.source != "registry" {
                    println!("  Source: {}", installed.source);
                }
            }
        }
        Commands::Uninstall { name } => {
            // Try uninstalling as skill first (since skills are separate from plugins)
            if sm.uninstall_skill(&name)? {
                println!("Uninstalled skill '{}'", name);
            } else if pm.uninstall(&name)? {
                println!("Uninstalled '{}' (plugin)", name);
            } else {
                eprintln!("Artifact '{}' is not installed", name);
            }
        }
        Commands::List => {
            // Show both plugins and skills with unified output
            // Plugins
            let lockfile = pm.load_lockfile();
            if !lockfile.plugins.is_empty() {
                println!("\nInstalled Plugins ({}):", lockfile.plugins.len());
                render_list_table(&lockfile).render();
            }

            // Skills
            let skill_lockfile = sm.load_skills_lockfile();
            if !skill_lockfile.skills.is_empty() {
                println!("\nInstalled Skills ({}):", skill_lockfile.skills.len());
                let mut t = Table::new(84)
                    .column(Column::fixed("NAME", 18))
                    .column(Column::fixed("VERSION", 10))
                    .column(Column::fixed("AUTHOR", 18))
                    .column(Column::flexible("DESCRIPTION", 30));
                for s in skill_lockfile.skills.values() {
                    let author = s.author.clone().unwrap_or_else(|| "-".to_string());
                    let desc = s.description.clone().unwrap_or_else(|| "-".to_string());
                    let short_desc = truncate_display(&desc, 28);
                    t.add_row(vec![s.name.clone(), s.version.clone(), author, short_desc]);
                }
                t.render();
            }

            let has_content = !lockfile.plugins.is_empty() || !skill_lockfile.skills.is_empty();
            if !has_content {
                println!(
                    "No artifacts installed. Use `rune install <name>` for plugins or `rune install <path>` for skills."
                );
                return Ok(());
            }
        }
        Commands::Available => {
            // Show both plugin and skill registries with unified output
            // Plugins
            let plugin_available = pm.fetch_registry().await?;
            let lockfile = pm.load_lockfile();
            if !plugin_available.is_empty() {
                println!(
                    "\nAvailable Plugins ({} in registry):",
                    plugin_available.len()
                );
                let mut t = Table::new(88)
                    .column(Column::fixed("NAME", 20))
                    .column(Column::fixed("CURRENT", 10))
                    .column(Column::fixed("LATEST", 10))
                    .column(Column::fixed("BUILDS", 14))
                    .column(Column::flexible("DESCRIPTION", 30));
                for p in &plugin_available {
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
                t.render();
            }

            // Skills
            let skill_available = sm.fetch_skills().await?;
            if !skill_available.is_empty() {
                println!(
                    "\nAvailable Skills ({} in registry):",
                    skill_available.len()
                );
                let mut t = Table::new(88)
                    .column(Column::fixed("NAME", 20))
                    .column(Column::fixed("CURRENT", 10))
                    .column(Column::fixed("LATEST", 10))
                    .column(Column::flexible("DESCRIPTION", 30));
                for s in &skill_available {
                    t.add_row(vec![
                        s.name.clone(),
                        s.latest.clone(),
                        s.latest.clone(),
                        s.description.clone().unwrap_or_else(|| "-".to_string()),
                    ]);
                }
                t.render();
            }

            let has_content = !plugin_available.is_empty() || !skill_available.is_empty();
            if !has_content {
                println!("No artifacts found in the registries.");
                return Ok(());
            }
        }
        Commands::Search { keyword } => {
            // Search both plugin and skill registries
            let kw_lower = keyword.to_lowercase();
            let mut matches: Vec<SearchResult> = Vec::new();

            // Search plugins
            if let Ok(available) = pm.fetch_registry().await {
                for p in available {
                    if p.name.to_lowercase().contains(&kw_lower)
                        || p.description
                            .as_deref()
                            .unwrap_or("")
                            .to_lowercase()
                            .contains(&kw_lower)
                    {
                        matches.push(SearchResult {
                            ty: "plugin".to_string(),
                            name: p.name.clone(),
                            latest: p.latest.clone(),
                            description: p.description.clone().unwrap_or_else(|| "-".to_string()),
                        });
                    }
                }
            }

            // Search skills
            if let Ok(available) = sm.fetch_skills().await {
                for s in available {
                    if s.name.to_lowercase().contains(&kw_lower)
                        || s.description
                            .as_deref()
                            .unwrap_or("")
                            .to_lowercase()
                            .contains(&kw_lower)
                        || s.tags.iter().any(|t| t.to_lowercase().contains(&kw_lower))
                    {
                        matches.push(SearchResult {
                            ty: "skill".to_string(),
                            name: s.name.clone(),
                            latest: s.latest.clone(),
                            description: s.description.clone().unwrap_or_else(|| "-".to_string()),
                        });
                    }
                }
            }

            if matches.is_empty() {
                println!("No artifacts found matching '{}'", keyword);
                return Ok(());
            }

            // Render with type prefix
            let mut t = Table::new(88)
                .column(Column::fixed("TYPE", 10))
                .column(Column::fixed("NAME", 20))
                .column(Column::fixed("CURRENT", 10))
                .column(Column::fixed("LATEST", 10))
                .column(Column::flexible("DESCRIPTION", 30));

            for item in &matches {
                let short_desc = truncate_display(&item.description, 28);
                t.add_row(vec![
                    item.ty.clone(),
                    item.name.clone(),
                    item.latest.clone(),
                    item.description.clone(),
                    short_desc,
                ]);
            }

            t.render();
        }
        Commands::Update {
            names,
            check,
            _reserved_for_future,
        } => {
            let filter = if names.is_empty() {
                None
            } else {
                Some(names.as_slice())
            };

            let plugin_statuses = pm.check_plugin_updates(filter).await?;
            let skill_statuses = sm.check_skill_updates(filter).await?;

            let all_statuses = plugin_statuses
                .into_iter()
                .map(|s| PluginUpdateStatus {
                    name: s.name,
                    has_wasm: s.has_wasm,
                    has_native: s.has_native,
                    installed_version: s.installed_version,
                    latest_version: s.latest_version,
                    has_update: s.has_update,
                })
                .chain(skill_statuses.into_iter().map(|s| PluginUpdateStatus {
                    name: s.name,
                    has_wasm: false,
                    has_native: false,
                    installed_version: s.installed_version,
                    latest_version: s.latest_version,
                    has_update: s.has_update,
                }))
                .collect::<Vec<_>>();

            if all_statuses.is_empty() {
                println!("No matching installed artifacts found to inspect or update.");
                return Ok(());
            }

            if check {
                render_update_status_table(&all_statuses).render();
                return Ok(());
            }

            let to_update: Vec<_> = all_statuses.into_iter().filter(|s| s.has_update).collect();
            if to_update.is_empty() {
                println!("All specified artifacts are already up to date.");
                return Ok(());
            }

            for item in to_update {
                // Determine if this is a plugin or skill based on name
                let is_skill = sm
                    .load_skills_lockfile()
                    .skills
                    .iter()
                    .any(|(n, _)| n.eq_ignore_ascii_case(&item.name));

                let ty = if is_skill { "skill" } else { "plugin" };
                println!(
                    "Updating {} '{}' (v{} -> v{})...",
                    ty, item.name, item.installed_version, item.latest_version
                );

                if is_skill {
                    let installed = sm
                        .install_skill(&item.name, Some(item.latest_version))
                        .await?;
                    println!(
                        "Successfully updated skill '{}' to v{}",
                        installed.name, installed.version
                    );
                } else {
                    let installed = pm
                        .install(&item.name, Some(item.latest_version), false)
                        .await?;
                    let kind_label =
                        match (&installed.native_binary_path, &installed.execution_kind) {
                            (Some(_), ExecutionKind::Native) => "native",
                            (Some(_), ExecutionKind::Wasm) => "wasm+native",
                            (None, ExecutionKind::Native) => "native",
                            (None, ExecutionKind::Wasm) => "wasm",
                        };
                    println!(
                        "Successfully updated plugin '{}' to v{} ({})",
                        installed.name, installed.version, kind_label
                    );
                }
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
    }

    Ok(())
}

fn resolve_host_env_params() -> HashMap<String, String> {
    let mut map = HashMap::new();
    for (k, v) in std::env::vars() {
        map.insert(k.to_ascii_lowercase(), v);
    }
    map
}
