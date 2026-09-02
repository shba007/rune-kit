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
    /// Update installed tools
    Update { name: Option<String> },
}

fn parse_key_val(s: &str) -> Result<(String, String), String> {
    s.split_once('=')
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .ok_or_else(|| format!("Invalid KEY=VALUE pair: {}", s))
}
