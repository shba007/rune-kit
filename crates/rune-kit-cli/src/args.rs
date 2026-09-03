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
    /// Install an MCP tool from registry, URL, or local path
    Install {
        target: String,
        #[arg(short, long)]
        version: Option<String>,
        /// Prefer a native binary sidecar build over WebAssembly when available
        #[arg(long)]
        native: bool,
    },
    /// Uninstall an installed tool
    Uninstall { name: String },
    /// List installed tools
    List,
    /// List all available tools in the remote registry
    Available,
    /// Search tools in the registry by keyword across names and descriptions
    Search {
        /// Search keyword to match against plugin name or description
        keyword: String,
    },
    /// Update installed tools from the registry or inspect update availability
    #[command(alias = "outdated")]
    Update {
        /// Specific plugins to update or inspect (updates all if omitted)
        names: Vec<String>,
        /// Check for available updates without downloading
        #[arg(short, long)]
        check: bool,
        /// Prefer native binary sidecar builds over WebAssembly
        #[arg(long)]
        native: bool,
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

fn parse_key_val(s: &str) -> Result<(String, String), String> {
    s.split_once('=')
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .ok_or_else(|| format!("Invalid KEY=VALUE pair: {}", s))
}
