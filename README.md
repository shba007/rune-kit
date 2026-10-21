<div align="center">

# Rune Kit

**High-performance Model Context Protocol (MCP) plugin and skill runtime host and package manager.**

[![Rust CI](https://github.com/shba007/rune-kit/actions/workflows/integrate.yml/badge.svg)](https://github.com/shba007/rune-kit/actions/workflows/integrate.yml)
[![Release](https://img.shields.io/github/v/release/shba007/rune-kit?include_prereleases&logo=github)](https://github.com/shba007/rune-kit/releases)
[![Rust Edition](https://img.shields.io/badge/rust-edition%202024-orange?logo=rust)](https://www.rust-lang.org)
[![MCP Protocol](https://img.shields.io/badge/MCP%20protocol-2025--03--26-blueviolet)](https://modelcontextprotocol.io)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

</div>

---

Rune Kit is a Rust runtime host and package manager for the [Model Context Protocol (MCP)](https://modelcontextprotocol.io) specification (`protocolVersion: 2025-03-26`). It ships as a single CLI binary (`rune`) and a modular core library (`rune-kit-core`) designed to run, supervise, and multiplex MCP plugins across WebAssembly sandboxes and direct OS native processes through a single stdio front door.

In addition to executable plugins, Rune Kit natively manages declarative content artifacts called **Skills** (instruction sets, system prompts, reference packages), and features a host-managed external binary provisioning engine that automatically fetches, verifies, and configures standalone CLI utilities (such as `ffmpeg` or `yt-dlp`) on clean machines without requiring third-party package managers.

See the [MCP gateway contract](docs/MCP-CONTRACT.md) for annotation compatibility, read-only policy, cancellation, timeouts and development checks.

### Features

- ⚡ **Dual Execution Model:** Run sandboxed WebAssembly guest modules (`wasm32-wasip1`) via Extism/Wasmtime, or execute standalone native binaries communicating over JSON-RPC stdio.
- 🌐 **Unified MCP Router:** Multiplex tools (`<namespace>__<tool>`), resources (`rune://<namespace>/<uri>`), and prompts across multiple plugins into a single stdio gateway.
- 📦 **Two-Tier Artifact Management:** Manage executable code plugins (`installed.json`) alongside declarative content skills (`installed_skills.json`) with automated dependency resolution.
- 🛠️ **Host-Managed Binary Engine:** Auto-download, SHA-256 verify, unpack, and inject external CLI utilities (`_bin/<name>/<version>/`) into plugin execution environments.
- 🔒 **Fine-Grained Capabilities:** Restrict network egress (`allowed_hosts`), sandbox filesystem access (`allowed_dir`), and allowlist external binary execution via declarative manifests (`plugin.toml`).
- 🔄 **Autonomous Self-Updates:** Update the `rune` binary directly from GitHub releases across Linux (GNU/musl), macOS (Apple Silicon/Intel), and Windows.

---

## Architecture Overview

```text
┌─────────────────────────────────────────────────────────────────┐
│                    MCP Client (Cursor, Claude, etc.)            │
└────────────────────────────────┬────────────────────────────────┘
                                 │ JSON-RPC (stdio)
                                 ▼
┌─────────────────────────────────────────────────────────────────┐
│                     McpRouter (protocol.rs)                     │
│  • Single JSON-RPC stdio transport front door                   │
│  • Tools: <namespace>__<tool_name>                              │
│  • Resources: rune://<namespace>/<uri>                          │
│  • Prompts: <namespace>__<prompt_name>                          │
└──────────────┬───────────────────────────────────┬──────────────┘
               │                                   │
               ▼                                   ▼
┌──────────────────────────────┐   ┌──────────────────────────────┐
│      WasmPluginInstance      │   │        NativeSidecar         │
│  • Extism / Wasmtime sandbox │   │    (Standalone Execution)    │
│  • WASI execution context    │   │  • Direct stdio JSON-RPC     │
│  • Scoped fs & net gating    │   │  • Hardware / OS / C-FFI     │
└──────────────┬───────────────┘   └──────────────────────────────┘
               │ Host Bridge IPC
               ▼
┌──────────────────────────────┐
│   Companion Native Sidecar   │
│   (rune-<name>-native)       │
│   • Heavy compute / native OS│
└──────────────┬───────────────┘
               │ Host FFI
               ▼
┌──────────────────────────────┐
│   Provisioned CLI Binaries   │
│   (_bin/<name>/<version>/)   │
│   • ffmpeg, yt-dlp, etc.     │
└──────────────────────────────┘
```

### Execution Tiers

| Execution Kind | Implementation | Protocol Transport | Primary Use Case |
| :--- | :--- | :--- | :--- |
| **Pure WASM** | `WasmPluginInstance` | Extism memory buffer export | Deterministic compute, API integrations, document transformers, sandboxed network & file tasks. |
| **WASM + Native Sidecar** | `WasmPluginInstance` + companion `NativeSidecar` | Extism host function `host_sidecar_exec` | Maintaining portable MCP interfaces in WASM while delegating intensive native operations to a background helper process. |
| **Direct Standalone Native** | `NativeSidecar` | Supervised OS child process (stdio JSON-RPC) | Window managers, accessibility APIs, GPU/display drivers, or precompiled MCP binaries requiring full OS integration. |

---

## Workspace Layout

| Crate | Type | Directory | Description |
| :--- | :--- | :--- | :--- |
| [`rune-kit-core`](file:///crates/core) | Library | `crates/core` | Core runtime host, `McpRouter`, Extism WASM engine, native sidecar supervision, artifact manager, and binary provisioner. |
| [`rune-kit-cli`](file:///crates/cli) | Binary & Library | `crates/cli` | The `rune` CLI binary (`src/main.rs`) and CLI library routines (`src/lib.rs`) for command parsing and table rendering. |

---

## Installation

### Prebuilt Binaries

Download precompiled release archives directly from [GitHub Releases](https://github.com/shba007/rune-kit/releases):

- **Linux (x86_64):** GNU (`x86_64-unknown-linux-gnu.tar.gz`) or Musl (`x86_64-unknown-linux-musl.tar.gz`)
- **macOS:** Apple Silicon (`aarch64-apple-darwin.tar.gz`) or Intel (`x86_64-apple-darwin.tar.gz`)
- **Windows (x86_64):** MSVC (`x86_64-pc-windows-msvc.zip`)

### Build from Source

Rune Kit requires **Rust 2024 edition** (Rust 1.85+):

```bash
# Clone the repository
git clone https://github.com/shba007/rune-kit.git
cd rune-kit

# Compile the workspace in release mode
cargo build --release

# Install the `rune` binary to your Cargo binary path (~/.cargo/bin)
cargo install --path crates/cli
```

Verify the installation:

```bash
rune --version
# Output: rune 0.2.2
```

---

## CLI Reference

```text
Usage: rune <COMMAND>

Commands:
  run          Start the MCP server gateway over stdio
  install      Install an artifact from registry, URL, or local path
  uninstall    Uninstall an installed artifact (plugin or skill)
  list         List installed artifacts (plugins and skills)
  available    List available artifacts in the remote registry
  search       Search artifacts in the registry by keyword across names and descriptions
  update       Update installed artifacts from the registry or inspect update availability
  self-update  Self-update the rune CLI binary to the latest release
  help         Print this message or the help of the given subcommand(s)
```

### 1. `rune run`

Starts the MCP server gateway over stdio. Dispatches JSON-RPC requests across one or all registered plugins.

```bash
# Run a single plugin by installed name
rune run git

# Run a local plugin directly (file extension determines runtime: .wasm -> WASM, other -> Native)
rune run ./dist/my-plugin.wasm
rune run ./dist/native-worker.exe

# Gateway Mode: Serve all installed plugins simultaneously under namespaced identifiers
rune run --all

# Inject key-value parameters passed to the plugin runtime
rune run git -p allowed_dir=/workspace -p default_branch=main
```

#### Flags & Options

| Flag | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `[PLUGIN]` | `String` (positional) | `None` | Specific plugin name or local file path to run. |
| `--all` | `bool` | `false` | Run all installed plugins simultaneously in gateway mode. |
| `-p`, `--param <KEY=VALUE>` | `Vec<String>` | `[]` | Injected key-value parameter passed to plugin configuration and environment. Repeatable. |
| `-h`, `--help` | `bool` | `false` | Print help information. |

> [!NOTE]
> Either `[PLUGIN]` or `--all` must be provided. If neither is specified, `rune run` exits with code `1`.

#### Environment Variable Parameter Injection

In addition to `-p KEY=VALUE`, `rune run` automatically inspects the host environment and forwards parameters into the plugin context:
1. **Prefixed Variables:** Any variable starting with `RUNE_PARAM_<NAME>` has the prefix stripped and the remaining key lowercased (e.g. `RUNE_PARAM_API_KEY=xyz` becomes `api_key=xyz`).
2. **Allowlisted System Variables:** The following host environment variables are forwarded automatically (lowercased): `ALLOWED_DIR`, `OUTPUT_DIR`, `OUTPUT_DIRECTORY`, `COOKIES_DIR`, `COOKIES_FILE`, `DEFAULT_TIMEZONE`, `REPO_PATH`, `MEMORY_FILE`, `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, `NO_PROXY`.

---

### 2. `rune install`

Installs a plugin or skill from the remote registry, an HTTP(S) URL, or a local file path.

```bash
# Install a plugin from the registry (defaulting to WASM, falls back if needed)
rune install git

# Prefer native companion/standalone binary if available
rune install browser --native

# Pin a specific version
rune install git --version 0.1.2

# Install from a local file (.wasm, .zip, .tar.gz, .exe)
rune install ./dist/rune_memory.wasm

# Install a declarative skill (prompts/instructions) from file or markdown bundle
rune install ./skills/code-review.md
rune install #prompt-engineering
```

#### Flags & Options

| Flag | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `<TARGET>` | `String` (positional) | *Required* | Registry name, HTTP/HTTPS URL, or local file path to install. |
| `-v`, `--version <VERSION>` | `String` | `None` (latest) | Specific release version to install. |
| `--native` | `bool` | `false` | Prefer installing native binary if available in the registry. |
| `-h`, `--help` | `bool` | `false` | Print help information. |

#### Artifact Differentiation (Plugins vs. Skills)

`rune install` automatically routes targets to the appropriate subsystem:
- **Skill Detection:** If `<TARGET>` ends with `.md`, `.txt`, `.rst`, `.markdown`, contains double underscores (`__`), starts with `#`, or matches an entry with `type: "skill"` in the registry, it is installed by `SkillManager` into `skills/<name>/`.
- **Plugin Detection:** All other targets route to `PackageManager` into `plugins/`. Any companion native binaries (`rune-<name>-native` or `<name>-native`) and `plugin.toml` files in the same directory are bundled alongside it.

---

### 3. `rune uninstall`

Removes an installed plugin or skill and purges its local binaries and lockfile entries.

```bash
rune uninstall git
rune uninstall code-review
```

| Flag | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `<NAME>` | `String` (positional) | *Required* | Name of the installed plugin or skill to remove. |
| `-h`, `--help` | `bool` | `false` | Print help information. |

---

### 4. `rune list`

Displays all installed plugins and skills in formatted terminal tables, including version, build types (`wasm`, `native`, or `wasm, native`), and sources.

```bash
rune list
```

---

### 5. `rune available`

Queries the remote registry and lists all available plugins and skills, displaying current local installed version vs. remote latest version.

```bash
rune available
```

---

### 6. `rune search`

Performs a case-insensitive search across names, descriptions, and tags in the remote registry.

```bash
rune search git
rune search prompt
```

| Flag | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `<KEYWORD>` | `String` (positional) | *Required* | Search keyword to match against artifact name, description, or tags. |
| `-h`, `--help` | `bool` | `false` | Print help information. |

---

### 7. `rune update` (alias: `outdated`)

Inspects or updates installed plugins and skills against the remote registry.

```bash
# Check for updates without downloading
rune update --check

# Update all outdated artifacts to the latest version
rune update

# Update only specific artifacts
rune update git memory
```

| Flag | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `[NAMES]...` | `Vec<String>` (positional) | `[]` (all) | Optional list of specific artifact names to inspect or update. |
| `-c`, `--check` | `bool` | `false` | Check for available updates and render the status table without downloading. |
| `-h`, `--help` | `bool` | `false` | Print help information. |

---

### 8. `rune self-update`

Checks for and updates the running `rune` CLI binary directly from GitHub releases matching the current OS target triple.

```bash
# Inspect whether a newer CLI binary exists
rune self-update --check

# Perform the update
rune self-update

# Force reinstall or pin a specific version
rune self-update --version 0.2.1 --force
```

| Flag | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `-c`, `--check` | `bool` | `false` | Check if an update is available without downloading. |
| `-v`, `--version <VERSION>` | `String` | `None` (latest) | Specific release version to target (e.g. `0.2.0`). |
| `-f`, `--force` | `bool` | `false` | Reinstall even if the current binary is already up to date. |
| `-h`, `--help` | `bool` | `false` | Print help information. |

---

## File System & Storage Layout

Rune Kit stores data under the standard platform data directory via [`dirs::data_dir()`](https://docs.rs/dirs/latest/dirs/fn.data_dir.html):

| Platform | Location |
| :--- | :--- |
| **Linux** | `~/.local/share/rune-kit/` |
| **macOS** | `~/Library/Application Support/rune-kit/` |
| **Windows** | `%APPDATA%\rune-kit\` (`C:\Users\<User>\AppData\Roaming\rune-kit\`) |

### Directory Structure

```text
<data_dir>/rune-kit/
├── installed.json                 # Lockfile tracking installed plugins, hashes, paths
├── installed_skills.json          # Lockfile tracking installed skills and file checksums
├── plugins/                       # Executable plugin binaries
│   ├── git-0.1.2.wasm             # Sandboxed WASM modules
│   └── rune-browser-native.exe    # Companion or standalone OS executables
├── skills/                        # Content packages & prompts
│   └── code-review/
│       ├── SKILL.md
│       └── references.md
└── _bin/                          # Host-managed external CLI dependencies
    ├── ffmpeg/
    │   └── 6.1.1/
    │       ├── ffmpeg.exe
    │       ├── ffprobe.exe
    │       └── .verified          # SHA-256 verification activation marker
    └── yt-dlp/
        └── 2024.03.10/
            ├── yt-dlp.exe
            └── .verified
```

---

## Programmatic API (`rune-kit-core`)

Add `rune-kit-core` to your `Cargo.toml`:

```toml
[dependencies]
rune-kit-core = { git = "https://github.com/shba007/rune-kit.git" }
tokio = { version = "1", features = ["full"] }
serde_json = "1.0"
```

### 1. Hosting an MCP Gateway over Stdio

Load WASM or native plugin instances and route MCP traffic:

```rust
use rune_kit_core::{McpRouter, PluginInstance};
use std::collections::HashMap;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut router = McpRouter::new();
    let mut params = HashMap::new();
    params.insert("allowed_dir".to_string(), "/workspace".to_string());

    // Load a WASM plugin instance
    let wasm_plugin = PluginInstance::load_wasm_from_file(
        "filesystem",
        "/path/to/filesystem.wasm",
        params.clone(),
    )?;
    router.register("fs".to_string(), wasm_plugin);

    // Load a direct Native sidecar instance
    let native_plugin = PluginInstance::load_native_from_file(
        "desktop",
        "/path/to/desktop_worker.exe",
        params,
    )?;
    router.register("desktop".to_string(), native_plugin);

    // Start synchronous stdio JSON-RPC loop (blocks until EOF on stdin)
    router.run_stdio()?;
    Ok(())
}
```

### 2. Direct JSON-RPC Dispatch

Handle in-memory or custom transport requests via `handle_jsonrpc`:

```rust
use rune_kit_core::{McpRouter, PluginInstance};
use serde_json::json;
use std::collections::HashMap;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut router = McpRouter::new();
    let instance = PluginInstance::load_wasm_from_file(
        "calculator",
        "calculator.wasm",
        HashMap::new(),
    )?;
    router.register("calc".to_string(), instance);

    // Tools list request
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list"
    })
    .to_string();

    if let Some(response) = router.handle_jsonrpc(&request) {
        println!("{}", serde_json::to_string_pretty(&response)?);
    }

    Ok(())
}
```

### 3. Programmatic Artifact Management

Manage plugins and skills using `PackageManager` and `SkillManager`:

```rust
use rune_kit_core::{PackageManager, SkillManager};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pm = PackageManager::new();
    let sm = SkillManager::new();

    // Query available plugins from the remote registry
    let available = pm.fetch_registry().await?;
    for plugin in available {
        println!("{}: latest v{}", plugin.name, plugin.latest);
    }

    // Install a plugin (specifying name, optional version, prefer_native)
    let installed_plugin = pm.install("git", None, false).await?;
    println!("Installed plugin at: {}", installed_plugin.binary_path);

    // Install a declarative content skill
    let installed_skill = sm.install_skill("code-review", None).await?;
    println!("Installed skill with {} files", installed_skill.files.len());

    Ok(())
}
```

---

## Plugin Manifest Contract (`plugin.toml`)

Plugins declare required execution capabilities and external CLI dependencies in an optional `plugin.toml` placed alongside the binary:

```toml
name = "media-processor"
version = "0.2.0"
description = "Extracts audio and video streams"

[capabilities]
network_hosts = ["api.example.com", "stream.media.org"]
filesystem = { mode = "scoped", root_param = "allowed_dir" }
exec = { allowed_binaries = ["ffmpeg", "ffprobe"] }

[dependencies.binaries.ffmpeg]
version = ">=6.1"
optional = false
description = "Audio transcoding and media extraction"
url = "https://github.com/eugeneware/ffmpeg-static/releases/download/b6.1/ffmpeg-linux-x64.tar.gz"
sha256 = "c3ab8ff13720e8ad9047dd39466b3c8974e592c2fa383d4a3960714caef0c4f2"
```

When provisioned, Rune Kit automatically populates `<NAME>_PATH` (e.g. `FFMPEG_PATH`) and `<name>_path` environment variables, and prepends the directory of provisioned binaries to `PATH` for both WASM guests and native sidecars.

---

## Development

<details>
<summary><strong>Local Development & Verification Tasks</strong></summary>

### Prerequisites
- [Rust](https://rustup.rs/) edition 2024 (v1.85+)
- Cargo toolchain with `clippy` and `rustfmt`

### Common Workflows

```bash
# Verify dependencies against locked Cargo.lock
cargo fetch --locked

# Build all workspace members
cargo build --workspace

# Run integration and unit tests
cargo test --workspace

# Lint code with Clippy
cargo clippy --workspace --all-targets -- -D warnings

# Format code with rustfmt
cargo fmt --all --check
```

</details>

---

## License

This project is licensed under the [MIT License](LICENSE).

Copyright (c) 2024 Shirsendu Bairagi.

[![Contributors](https://contrib.rocks/image?repo=shba007/rune-kit)](https://github.com/shba007/rune-kit/graphs/contributors)
