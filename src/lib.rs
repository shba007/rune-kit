use clap::{Parser, Subcommand};
use serde_json::json;

#[derive(Parser)]
#[command(
    name = "rune-kit",
    version,
    about = "Rune MCP Engine & Package Manager"
)]
pub struct Cli {
    /// Output response as machine-readable JSON
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Run first-run setup again
    Setup,
    /// Manage third-party Streamable HTTP MCP servers
    Remote {
        #[command(subcommand)]
        action: remotes::Action,
    },
    /// Review staged changes and explicitly approve copy-back or recovery
    Changes {
        plugin: String,
        #[command(subcommand)]
        action: ChangesAction,
    },
    /// Start the persistent local MCP HTTP server and attached terminal dashboard
    Serve {
        /// Optional installed plugin name or artifact path (default: all installed)
        plugin: Option<String>,
        #[arg(long, conflicts_with = "plugin")]
        all: bool,
        #[arg(long, default_value = "127.0.0.1")]
        bind: std::net::IpAddr,
        #[arg(long, default_value_t = 47601)]
        port: u16,
        /// Run without a terminal dashboard
        #[arg(long)]
        headless: bool,
        /// Isolate WASM files to per-plugin workspaces; block native plugins and helpers
        #[arg(long)]
        workspace_only: bool,
        #[arg(skip)]
        selected: Vec<String>,
        #[arg(short, long, value_parser = parse_key_val)]
        param: Vec<(String, String)>,
        #[arg(long)]
        read_only: bool,
        #[arg(long = "allow-tool", value_delimiter = ',')]
        allow_tool: Vec<String>,
        /// Tool execution deadline in seconds
        #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..))]
        timeout: u64,
    },
    /// Removed public stdio command; use serve --headless
    #[command(hide = true)]
    Run {
        /// Run a specific plugin by name or local file path
        plugin: Option<String>,
        /// Run all installed plugins simultaneously in gateway mode
        #[arg(long)]
        all: bool,
        /// Injected key-value parameter (e.g. -p allowed_dir=/workspace)
        #[arg(short, long, value_parser = parse_key_val)]
        param: Vec<(String, String)>,
        /// Run in read-only mode, exposing only tools declared read-only
        #[arg(long)]
        read_only: bool,
        /// Allowlist specific tools (e.g. --allow-tool filesystem__read_file)
        #[arg(long = "allow-tool", value_delimiter = ',')]
        allow_tool: Vec<String>,
        /// Per-tool execution timeout in seconds
        #[arg(long)]
        timeout: Option<u64>,
    },
    /// Install an artifact from registry, URL, or local path
    Install {
        target: String,
        #[arg(short, long)]
        version: Option<String>,
        /// Prefer installing native binary if available
        #[arg(long)]
        native: bool,
    },
    /// Uninstall an installed artifact (plugin or skill)
    Uninstall { name: String },
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
    },
    /// Self-update the rune-kit CLI binary to the latest release
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

#[derive(Debug, Clone)]
struct SearchResult {
    ty: String,
    name: String,
    latest: String,
    description: String,
}

#[derive(Subcommand)]
pub enum ChangesAction {
    List,
    Apply {
        path: std::path::PathBuf,
        #[arg(long)]
        approval: String,
    },
    Backups,
    Restore {
        id: String,
        #[arg(long)]
        approval: String,
    },
}

fn parse_key_val(s: &str) -> Result<(String, String), String> {
    s.split_once('=')
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .ok_or_else(|| format!("Invalid KEY=VALUE pair: {}", s))
}

pub mod file_access;
pub mod headers;
pub mod manifest;
pub mod native_isolation;
pub mod package;
pub mod paths;
pub mod protocol;
pub mod provision;
pub mod registry;
pub mod remote;
pub mod runtime;
pub mod sandbox;

pub use manifest::{
    BinaryDependencySpec, Capabilities, ExecCapability, ExecutionKind, FilesystemCapability,
    InstalledMcp, InstalledPlugin, InstalledSkill, Lockfile, PluginInfo, PluginManifest,
    PluginUpdateStatus, PromptArgument, PromptDefinition, RegistryPluginSummary,
    RegistrySkillSummary, ResourceDefinition, SkillFile, SkillLockfile, SkillRow, ToolAnnotations,
    ToolDefinition, ToolRuleSpec,
};
pub use package::{PackageError, PackageManager, SkillManager, is_newer_version};
pub use protocol::{McpRouter, RouterConfig};
pub use provision::{BinaryProvisioner, ProvisionError};
pub use runtime::{
    NativeSidecar, PluginInstance, RuntimeError, WasmPluginInstance, resolve_host_env_params,
};

extern crate self as rune_kit;
extern crate self as rune_kit_cli;
extern crate self as rune_kit_core;

mod changes_panel;
mod config_editor;
pub mod configuration;
pub mod output;
mod remote_panel;
pub mod remotes;
mod sandbox_panel;
pub mod server;
mod setup;
pub mod tui;

use crate::output::{
    Column, Table, render_list_table, render_update_status_table, truncate_display,
};

pub async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    rune_kit_core::paths::managed_root()?;
    let pm = PackageManager::new();
    let sm = SkillManager::new();

    let command = match cli.command {
        Some(Commands::Setup) => setup::run().await?,
        Some(command) => command,
        None => match setup::saved_command()? {
            Some(command) => command,
            None => setup::run().await?,
        },
    };
    match command {
        Commands::Setup=>unreachable!("setup is resolved before dispatch"),
        Commands::Remote { action } => remotes::run(action, &rune_kit_core::paths::managed_root()?, cli.json)?,
        Commands::Changes{plugin,action}=>{
            let broker=rune_kit_core::file_access::Broker::open(rune_kit_core::paths::sandbox_root()?,&plugin)?;
            broker.prune_completed()?;
            match action {
                ChangesAction::List=>{
                    let changes=broker.reviews()?.into_iter().map(|review|Ok(json!({"approval":review.approval_token()?,"change":review}))).collect::<Result<Vec<_>,std::io::Error>>()?;
                    println!("{}",serde_json::to_string_pretty(&changes)?);
                }
                ChangesAction::Apply{path,approval}=>{
                    let review=broker.review_for(&path)?;
                    if approval!=review.approval_token()? {return Err("Approval does not match the current review; list and review changes again".into());}
                    broker.apply_review(&review)?;
                    println!("Changes copied back; recovery backup retained for seven days.");
                }
                ChangesAction::Backups=>{
                    let backups=broker.recoveries()?.into_iter().map(|info|Ok(json!({"approval":info.approval_token()?,"recovery":info}))).collect::<Result<Vec<_>,std::io::Error>>()?;
                    println!("{}",serde_json::to_string_pretty(&backups)?);
                }
                ChangesAction::Restore{id,approval}=>{
                    let info=broker.recoveries()?.into_iter().find(|i|i.id==id).ok_or("Recovery record not found")?;
                    if approval!=info.approval_token()? {return Err("Approval does not match the recovery record".into());}
                    broker.restore(&info)?;
                    println!("Original restored from the recovery backup.");
                }
            }
        }
        Commands::Run { .. } => return Err("Public stdio serving was removed. Use `rune-kit serve --headless` and connect to http://127.0.0.1:47601/mcp (MCP HTTP 2026-07-28; Kit 0.6.0+).".into()),
        Commands::Serve {
            plugin,
            all,
            param,
            read_only,
            allow_tool,
            timeout, bind, port, headless, workspace_only, selected,
        } => {
            server::validate_bind(bind)?;
            let selected_targets: Vec<String> = if !selected.is_empty() {
                selected
            } else if let Some(target) = plugin {
                if !all { vec![target] } else { vec![] }
            } else {
                vec![]
            };
            let (router, options, dashboard_config) = load_installed_environment(
                &selected_targets,
                read_only,
                timeout,
                workspace_only,
                &allow_tool,
                &param,
                bind,
                port,
                headless,
            )?;
            server::serve(router, options, dashboard_config).await?;
        }

        Commands::Install {
            target,
            version,
            native,
        } => {
            let is_file_or_hash_skill = target.ends_with(".md")
                || target.ends_with(".txt")
                || target.ends_with(".rst")
                || target.ends_with(".markdown")
                || target.contains("__")
                || target.starts_with('#');

            let is_skill = is_file_or_hash_skill || {
                if let Ok(skills) = sm.fetch_skills().await {
                    skills.iter().any(|s| s.name.eq_ignore_ascii_case(&target))
                } else {
                    false
                }
            };

            if is_skill {
                let installed = sm.install_skill(&target, version).await?;
                if cli.json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&json!({
                            "status": "success",
                            "type": "skill",
                            "name": installed.name,
                            "version": installed.version,
                            "author": installed.author,
                            "description": installed.description,
                            "source": installed.source
                        }))?
                    );
                } else {
                    println!(
                        "Installed skill '{}' (v{}, {}) — {}",
                        installed.name,
                        installed.version,
                        installed.source,
                        installed.description.unwrap_or_else(|| "-".to_string()),
                    );
                }
            } else {
                let installed = pm.install(&target, version, native).await?;
                let kind_label = match (&installed.native_binary_path, &installed.execution_kind) {
                    (Some(_), ExecutionKind::Native) => "native",
                    (Some(_), ExecutionKind::Wasm) => "wasm+native",
                    (None, ExecutionKind::Native) => "native",
                    (None, ExecutionKind::Wasm) => "wasm",
                };

                if cli.json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&json!({
                            "status": "success",
                            "type": "plugin",
                            "name": installed.name,
                            "version": installed.version,
                            "execution_kind": kind_label,
                            "description": installed.description,
                            "source": installed.source
                        }))?
                    );
                } else {
                    println!(
                        "Installed '{}' (v{}, {})",
                        installed.name, installed.version, kind_label
                    );

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
        }
        Commands::Uninstall { name } => {
            if sm.uninstall_skill(&name)? {
                if cli.json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&json!({
                            "status": "success",
                            "type": "skill",
                            "name": name,
                            "action": "uninstalled"
                        }))?
                    );
                } else {
                    println!("Uninstalled skill '{}'", name);
                }
            } else if pm.uninstall(&name)? {
                if cli.json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&json!({
                            "status": "success",
                            "type": "plugin",
                            "name": name,
                            "action": "uninstalled"
                        }))?
                    );
                } else {
                    println!("Uninstalled '{}' (plugin)", name);
                }
            } else {
                if cli.json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&json!({
                            "status": "error",
                            "message": format!("Artifact '{}' is not installed", name)
                        }))?
                    );
                    std::process::exit(1);
                } else {
                    eprintln!("Artifact '{}' is not installed", name);
                }
            }
        }
        Commands::List => {
            let lockfile = pm.load_lockfile();
            let skill_lockfile = sm.load_skills_lockfile();

            if cli.json {
                let plugins_json: Vec<_> = lockfile
                    .mcps
                    .values()
                    .map(|p| {
                        let kind_label = match (&p.native_binary_path, &p.execution_kind) {
                            (Some(_), ExecutionKind::Native) => "native",
                            (Some(_), ExecutionKind::Wasm) => "wasm+native",
                            (None, ExecutionKind::Native) => "native",
                            (None, ExecutionKind::Wasm) => "wasm",
                        };
                        json!({
                            "name": p.name,
                            "version": p.version,
                            "description": p.description,
                            "execution_kind": kind_label,
                            "binary_path": p.binary_path,
                            "native_binary_path": p.native_binary_path,
                            "source": p.source,
                        })
                    })
                    .collect();

                let skills_json: Vec<_> = skill_lockfile
                    .skills
                    .values()
                    .map(|s| {
                        json!({
                            "name": s.name,
                            "version": s.version,
                            "author": s.author,
                            "description": s.description,
                            "source": s.source,
                        })
                    })
                    .collect();

                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "plugins": plugins_json,
                        "skills": skills_json,
                    }))?
                );
                return Ok(());
            }

            if !lockfile.mcps.is_empty() {
                println!("\nInstalled MCP Servers ({}):", lockfile.mcps.len());
                render_list_table(&lockfile).render();
            }

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

            let has_content = !lockfile.mcps.is_empty() || !skill_lockfile.skills.is_empty();
            if !has_content {
                println!(
                    "No artifacts installed. Use `rune-kit install <name>` for MCPs or `rune-kit install <path>` for skills."
                );
                return Ok(());
            }
        }
        Commands::Available => {
            let plugin_available = pm.fetch_registry().await?;
            let lockfile = pm.load_lockfile();
            let skill_available = sm.fetch_skills().await?;
            let skill_lockfile = sm.load_skills_lockfile();

            if cli.json {
                let plugins_json: Vec<_> = plugin_available
                    .iter()
                    .map(|p| {
                        let current = lockfile.mcps.get(&p.name).map(|e| e.version.as_str());
                        json!({
                            "name": p.name,
                            "installed_version": current,
                            "latest_version": p.latest,
                            "has_wasm": p.has_wasm,
                            "has_native": p.has_native,
                            "description": p.description,
                        })
                    })
                    .collect();

                let skills_json: Vec<_> = skill_available
                    .iter()
                    .map(|s| {
                        let current = skill_lockfile
                            .skills
                            .get(&s.name)
                            .map(|e| e.version.as_str());
                        json!({
                            "name": s.name,
                            "installed_version": current,
                            "latest_version": s.latest,
                            "author": s.author,
                            "description": s.description,
                        })
                    })
                    .collect();

                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "plugins": plugins_json,
                        "skills": skills_json,
                    }))?
                );
                return Ok(());
            }

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
                        .mcps
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
                    let current = skill_lockfile
                        .skills
                        .get(&s.name)
                        .map(|e| e.version.as_str())
                        .unwrap_or("-");
                    t.add_row(vec![
                        s.name.clone(),
                        current.to_string(),
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
            let kw_lower = keyword.to_lowercase();
            let mut matches: Vec<SearchResult> = Vec::new();

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

            let lockfile = pm.load_lockfile();
            let skill_lockfile = sm.load_skills_lockfile();

            if cli.json {
                let results_json: Vec<_> = matches
                    .iter()
                    .map(|item| {
                        let current = if item.ty == "skill" {
                            skill_lockfile
                                .skills
                                .get(&item.name)
                                .map(|s| s.version.as_str())
                        } else {
                            lockfile.mcps.get(&item.name).map(|p| p.version.as_str())
                        };
                        json!({
                            "type": item.ty,
                            "name": item.name,
                            "installed_version": current,
                            "latest_version": item.latest,
                            "description": item.description,
                        })
                    })
                    .collect();

                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "keyword": keyword,
                        "results": results_json,
                    }))?
                );
                return Ok(());
            }

            if matches.is_empty() {
                println!("No artifacts found matching '{}'", keyword);
                return Ok(());
            }

            let mut t = Table::new(88)
                .column(Column::fixed("TYPE", 8))
                .column(Column::fixed("NAME", 20))
                .column(Column::fixed("CURRENT", 10))
                .column(Column::fixed("LATEST", 10))
                .column(Column::flexible("DESCRIPTION", 36));

            for item in &matches {
                let current = if item.ty == "skill" {
                    skill_lockfile
                        .skills
                        .get(&item.name)
                        .map(|s| s.version.as_str())
                        .unwrap_or("-")
                } else {
                    lockfile
                        .mcps
                        .get(&item.name)
                        .map(|p| p.version.as_str())
                        .unwrap_or("-")
                };

                let short_desc = truncate_display(&item.description, 34);
                t.add_row(vec![
                    item.ty.clone(),
                    item.name.clone(),
                    current.to_string(),
                    item.latest.clone(),
                    short_desc,
                ]);
            }

            t.render();
        }
        Commands::Update { names, check } => {
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

            if check {
                if cli.json {
                    let updates_json: Vec<_> = all_statuses
                        .iter()
                        .map(|s| {
                            json!({
                                "name": s.name,
                                "installed_version": s.installed_version,
                                "latest_version": s.latest_version,
                                "has_update": s.has_update,
                                "has_wasm": s.has_wasm,
                                "has_native": s.has_native,
                            })
                        })
                        .collect();

                    println!(
                        "{}",
                        serde_json::to_string_pretty(&json!({
                            "updates": updates_json,
                        }))?
                    );
                    return Ok(());
                }

                if all_statuses.is_empty() {
                    println!("No matching installed artifacts found to inspect or update.");
                    return Ok(());
                }
                render_update_status_table(&all_statuses).render();
                return Ok(());
            }

            if all_statuses.is_empty() {
                if cli.json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&json!({
                            "updated": []
                        }))?
                    );
                    return Ok(());
                }
                println!("No matching installed artifacts found to inspect or update.");
                return Ok(());
            }

            let to_update: Vec<_> = all_statuses.into_iter().filter(|s| s.has_update).collect();
            if to_update.is_empty() {
                if cli.json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&json!({
                            "updated": []
                        }))?
                    );
                    return Ok(());
                }
                println!("All specified artifacts are already up to date.");
                return Ok(());
            }

            let mut updated_items = Vec::new();
            for item in to_update {
                let is_skill = sm
                    .load_skills_lockfile()
                    .skills
                    .iter()
                    .any(|(n, _)| n.eq_ignore_ascii_case(&item.name));

                let ty = if is_skill { "skill" } else { "plugin" };
                if !cli.json {
                    println!(
                        "Updating {} '{}' (v{} -> v{})...",
                        ty, item.name, item.installed_version, item.latest_version
                    );
                }

                if is_skill {
                    let installed = sm
                        .install_skill(&item.name, Some(item.latest_version.clone()))
                        .await?;
                    if !cli.json {
                        println!(
                            "Successfully updated skill '{}' to v{}",
                            installed.name, installed.version
                        );
                    }
                    updated_items.push(json!({
                        "type": "skill",
                        "name": installed.name,
                        "old_version": item.installed_version,
                        "new_version": installed.version,
                        "status": "updated"
                    }));
                } else {
                    let installed = pm
                        .install(&item.name, Some(item.latest_version.clone()), false)
                        .await?;
                    let kind_label =
                        match (&installed.native_binary_path, &installed.execution_kind) {
                            (Some(_), ExecutionKind::Native) => "native",
                            (Some(_), ExecutionKind::Wasm) => "wasm+native",
                            (None, ExecutionKind::Native) => "native",
                            (None, ExecutionKind::Wasm) => "wasm",
                        };
                    if !cli.json {
                        println!(
                            "Successfully updated plugin '{}' to v{} ({})",
                            installed.name, installed.version, kind_label
                        );
                    }
                    updated_items.push(json!({
                        "type": "plugin",
                        "name": installed.name,
                        "old_version": item.installed_version,
                        "new_version": installed.version,
                        "execution_kind": kind_label,
                        "status": "updated"
                    }));
                }
            }

            if cli.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "updated": updated_items,
                    }))?
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
                if cli.json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&json!({
                            "current_version": format!("v{}", current_ver),
                            "target_release": tag,
                            "has_update": is_newer_version(latest_clean, current_ver),
                        }))?
                    );
                    return Ok(());
                }

                println!("Current version: v{}", current_ver);
                println!("Release target:  {}", tag);
                if is_newer_version(latest_clean, current_ver) {
                    println!("\nUpdate available! Run `rune-kit self-update` to install.");
                } else if latest_clean == current_ver {
                    println!("\nYou are already on the latest version.");
                } else {
                    println!("\nCurrent version is newer than the targeted release.");
                }
                return Ok(());
            }

            if !cli.json {
                println!("Checking for CLI release...");
            }
            match pm.self_update(version.as_deref(), force).await {
                Ok(new_tag) => {
                    if cli.json {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&json!({
                                "status": "success",
                                "updated_version": new_tag,
                            }))?
                        );
                    } else {
                        println!("Successfully updated rune-kit CLI to {}!", new_tag);
                    }
                }
                Err(rune_kit_core::PackageError::SelfUpdate(msg)) => {
                    if cli.json {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&json!({
                                "status": "error",
                                "message": msg,
                            }))?
                        );
                    } else {
                        eprintln!("{}", msg);
                    }
                }
                Err(err) => {
                    if cli.json {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&json!({
                                "status": "error",
                                "message": err.to_string(),
                            }))?
                        );
                    } else {
                        eprintln!("Failed to perform self-update: {}", err);
                    }
                }
            }
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn load_installed_environment(
    selected: &[String],
    read_only: bool,
    timeout: u64,
    workspace_only: bool,
    allow_tool: &[String],
    param: &[(String, String)],
    bind: std::net::IpAddr,
    port: u16,
    headless: bool,
) -> Result<(McpRouter, server::Options, configuration::Configuration), Box<dyn std::error::Error>>
{
    let pm = PackageManager::new();
    let allow_tool_for_dashboard = allow_tool.to_vec();
    let mut router = McpRouter::new();
    router.set_read_only(read_only);
    if !allow_tool.is_empty() {
        router.set_allowed_tools(allow_tool.to_vec());
    }
    router.set_tool_timeout(timeout);
    let mut server_params = resolve_host_env_params();
    server_params.extend(param.iter().cloned());
    let metadata_path = rune_kit_core::paths::managed_root()?.join("installed.json");
    let lockfile: rune_kit_core::Lockfile = match std::fs::read(&metadata_path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
            format!(
                "Cannot parse {}: {e}. Repair installed metadata before serving.",
                metadata_path.display()
            )
        })?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Default::default(),
        Err(e) => return Err(format!("Cannot read {}: {e}", metadata_path.display()).into()),
    };
    let remotes = remotes::read(&metadata_path.parent().unwrap().join("remotes.json"))?;
    if remotes.keys().any(|name| lockfile.mcps.contains_key(name)) {
        return Err("A remote and installed plugin share a namespace; remove or rename the remote before serving".into());
    }
    let targets: Vec<String> = if !selected.is_empty() {
        selected.to_vec()
    } else {
        let mut names: Vec<_> = lockfile
            .mcps
            .keys()
            .chain(remotes.keys())
            .cloned()
            .collect();
        names.sort();
        names
    };
    let mut dashboard_config =
        configuration::Configuration::read(metadata_path.parent().unwrap().join("dashboard.json"))?;
    if workspace_only && targets.iter().any(|target| remotes.contains_key(target)) {
        return Err("Workspace-only mode blocks external MCP servers: Kit cannot enforce their filesystem access. Serve a remote explicitly without --workspace-only.".into());
    }
    if workspace_only
        && targets.iter().any(|target| {
            lockfile.mcps.get(target).is_some_and(|p| {
                p.execution_kind == ExecutionKind::Native
                    && !rune_kit_core::native_isolation::VALIDATED_ON_ALL_PLATFORMS
            }) || (!lockfile.mcps.contains_key(target) && !target.ends_with(".wasm"))
        })
    {
        return Err("Workspace-only mode blocks native plugins. Select a WASM plugin explicitly, or omit --workspace-only for unenforced execution.".into());
    }
    let sandbox = if workspace_only {
        Some(rune_kit_core::sandbox::Sandbox::open(
            rune_kit_core::paths::sandbox_root()?,
        )?)
    } else {
        None
    };
    let mut loaded = Vec::new();
    let mut roots = Vec::new();
    for target in &targets {
        if let Some(config) = remotes.get(target) {
            let token = config.token_env.as_ref().map(|key|std::env::var(key).map_err(|_|format!("Remote {target}: configured bearer-token environment variable is missing"))).transpose()?;
            let instance =
                rune_kit_core::remote::RemoteHttp::new(target.clone(), config.clone(), token)?;
            loaded.push(format!(
                "{target}: external HTTP server loaded; discovery pending; filesystem not managed"
            ));
            router.register(target.clone(), PluginInstance::Remote(Box::new(instance)));
            continue;
        }
        let mut params = lockfile
            .mcps
            .get(target)
            .map(|entry| entry.default_params.clone())
            .unwrap_or_default();
        params.extend(setup::plugin_secrets(target)?);
        params.extend(server_params.clone());
        if !workspace_only && let Some(root) = params.get("allowed_dir") {
            roots.push(format!("{target}: {root}"));
        }
        let (path, kind, version) = if let Some(entry) = lockfile.mcps.get(target) {
            (
                pm.get_plugin_path(target).ok_or_else(|| {
                    format!(
                        "Installed plugin {target} has no payload. Reinstall it before serving."
                    )
                })?,
                entry.execution_kind.clone(),
                entry.version.clone(),
            )
        } else {
            let path = std::path::PathBuf::from(target);
            let kind = if path.extension().is_some_and(|v| v == "wasm") {
                ExecutionKind::Wasm
            } else {
                ExecutionKind::Native
            };
            (path, kind, "local artifact".into())
        };
        let spec = configuration::Plugin {
            name: target.clone(),
            path: path.clone(),
            kind: kind.clone(),
            params: params.clone(),
            workspace: sandbox.as_ref().map(|s| s.workspace(target)).transpose()?,
            manifest: lockfile
                .mcps
                .get(target)
                .and_then(|p| p.manifest.clone())
                .or_else(|| {
                    path.parent().and_then(|parent| {
                        rune_kit_core::PluginManifest::from_file(parent.join("plugin.toml")).ok()
                    })
                })
                .unwrap_or_default(),
        };
        if let Some(workspace) = &spec.workspace {
            roots.push(format!("{target}: /workspace ({})", workspace.display()));
        }
        let instance = if workspace_only || lockfile.mcps.contains_key(target) {
            spec.load(dashboard_config.settings())
                .map_err(|e| format!("Cannot load plugin {target}: {e}"))?
        } else {
            match kind {
                ExecutionKind::Wasm => PluginInstance::load_wasm_from_file(target, path, params),
                ExecutionKind::Native => {
                    PluginInstance::load_native_from_file(target, path, params)
                }
            }
            .map_err(|e| format!("Cannot load plugin {target}: {e}"))?
        };
        dashboard_config.plugins.push(spec);
        loaded.push(format!(
            "{target} {version}: loaded ({kind:?}); discovery pending"
        ));
        if let Some(manifest) = lockfile
            .mcps
            .get(target)
            .and_then(|entry| entry.manifest.as_ref())
        {
            for dependency in manifest.dependencies.binaries.keys() {
                let directory = metadata_path
                    .parent()
                    .unwrap()
                    .join("_bin")
                    .join(dependency);
                let verified = std::fs::read_dir(directory).is_ok_and(|entries| {
                    entries.flatten().any(|entry| {
                        entry.path().join(".verified").is_file()
                            && (entry.path().join(dependency).is_file()
                                || entry.path().join(format!("{dependency}.exe")).is_file())
                    })
                });
                if !verified {
                    loaded.push(format!("{target}: dependency {dependency} is not verified locally. Reinstall {target} to provision it; optional dependencies may be unused."));
                }
            }
        }
        router.register(target.clone(), instance);
    }
    let options = server::Options {
        bind,
        port,
        headless,
        workspace_only,
        read_only,
        allowed_tools: allow_tool_for_dashboard,
        timeout,
        plugins: loaded,
        roots,
    };
    Ok((router, options, dashboard_config))
}
