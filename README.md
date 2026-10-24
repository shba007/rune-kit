<div align="center">

<p align="center">
  <img src="./public/logo.svg" alt="Rune Kit ASCII logo" width="180" height="120">
</p>

# Rune Kit

**High-performance Model Context Protocol (MCP) plugin and skill runtime host and package manager.**

[![Rust CI](https://github.com/shba007/rune-kit/actions/workflows/integrate.yml/badge.svg)](https://github.com/shba007/rune-kit/actions/workflows/integrate.yml)
[![Release](https://img.shields.io/github/v/release/shba007/rune-kit?include_prereleases&logo=github)](https://github.com/shba007/rune-kit/releases)
[![Rust Edition](https://img.shields.io/badge/rust-edition%202024-orange?logo=rust)](https://www.rust-lang.org)
[![MCP Protocol](https://img.shields.io/badge/MCP%20protocol-2026--07--28-blueviolet)](https://modelcontextprotocol.io)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

</div>

---

Rune Kit is a Rust runtime host and package manager for the [Model Context Protocol (MCP)](https://modelcontextprotocol.io) specification (`per-request protocolVersion: 2026-07-28`). It ships as a single CLI binary (`rune-kit`) and a modular core library (`rune-kit-core`) designed to run, supervise, and multiplex MCP plugins across WebAssembly sandboxes and direct OS native processes through a persistent local Streamable HTTP endpoint.

In addition to executable plugins, Rune Kit natively manages declarative content artifacts called **Skills** (instruction sets, system prompts, reference packages), and features a host-managed external binary provisioning engine that automatically fetches, verifies, and configures standalone CLI utilities (such as `ffmpeg` or `yt-dlp`) on clean machines without requiring third-party package managers.

See the [MCP gateway contract](docs/MCP-CONTRACT.md) for annotation compatibility, read-only policy, cancellation, timeouts and development checks.

### Features

- ⚡ **Dual Execution Model:** Run sandboxed WebAssembly guest modules (`wasm32-wasip1`) via Extism/Wasmtime, or execute standalone native binaries communicating over JSON-RPC stdio.
- 🌐 **Unified MCP Router:** Multiplex tools (`<namespace>__<tool>`), resources (`rune://<namespace>/<uri>`), and prompts across multiple plugins into one HTTP MCP server.
- 📦 **Two-Tier Artifact Management:** Manage executable code plugins (`installed.json`) alongside declarative content skills (`installed_skills.json`) with automated dependency resolution.
- 🛠️ **Host-Managed Binary Engine:** Auto-download, SHA-256 verify, unpack, and inject external CLI utilities (`_bin/<name>/<version>/`) into plugin execution environments.
- 🔒 **Fine-Grained Capabilities:** Restrict network egress (`allowed_hosts`), sandbox filesystem access (`allowed_dir`), and allowlist external binary execution via declarative manifests (`plugin.toml`).
- 🔄 **Autonomous Self-Updates:** Update the `rune-kit` binary directly from GitHub releases across Linux (GNU/musl), macOS (Apple Silicon/Intel), and Windows.

---

## Architecture Overview

```text
┌─────────────────────────────────────────────────────────────────┐
│                    MCP Client (Cursor, Claude, etc.)            │
└────────────────────────────────┬────────────────────────────────┘
                                 │ MCP Streamable HTTP
                                 ▼
┌─────────────────────────────────────────────────────────────────┐
│                     McpRouter (protocol.rs)                     │
│  • Local MCP HTTP endpoint /mcp                   │
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
| [`rune-kit-cli`](file:///crates/cli) | Binary & Library | `crates/cli` | The `rune-kit` CLI binary (`src/main.rs`) and CLI library routines (`src/lib.rs`) for command parsing and table rendering. |

---

## Installation

### Install script

Install release 0.4.0 or newer without Rust or administrator rights:

Windows (PowerShell 5.1 or newer):

```powershell
irm https://raw.githubusercontent.com/shba007/rune-kit/main/install.ps1 | iex
```

Linux / macOS:

```sh
curl -fsSL https://raw.githubusercontent.com/shba007/rune-kit/main/install.sh | sh
```

Windows installs `rune-kit.exe` in `%USERPROFILE%\rune` and adds
that folder to your user PATH and the current PowerShell session. Linux/macOS
install `rune-kit` in `~/.rune`; the script adds the root once to `~/.profile` (set `RUNE_KIT_NO_MODIFY_PATH=1` to leave PATH unchanged).
Re-running installs the selected release again. Downloads are verified before an
existing executable is replaced. Stop active gateways before upgrading on Windows.

Override `RUNE_KIT_VERSION` with an existing release tag, `RUNE_KIT_INSTALL_DIR`, or
`RUNE_KIT_BASE_URL` (release-download URL or local mirror directory). A mirror
requires an explicit version and an archive plus its `.sha256` file. On Windows,
`RUNE_KIT_NO_MODIFY_PATH=1` leaves both user and session PATH unchanged.

For example, to pin a version in PowerShell:

```powershell
$env:RUNE_KIT_VERSION = 'v0.6.0' # HTTP release; use only after it is published
# Run the installer after it is published.
```

Windows also accepts older `rune-<tag>-<target>.zip` assets and verifies their
GitHub SHA-256 digest when no checksum sidecar exists, installing the executable
as `rune-kit.exe`. Older binaries still print their original `rune <version>`.
Linux/macOS require checksum sidecars, introduced with the renamed release.

**Migration:** change MCP client commands from `rune` to `rune-kit` and restart
terminals/clients after installing. No `rune` alias is installed and unrelated old
executables are not deleted. Plugin, skill and cache data use the
`~/.rune/tools` folder. Legacy managed storage is copied on first use; originals remain available for recovery. Old installs can be removed
manually after clients have switched. Self-update accepts both old and new archive
binary names. HTTP clients need the planned 0.6.0+ release and the Streamable HTTP request contract below; the published 0.5.0 stdio release is not HTTP-compatible.

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

# Install the `rune-kit` binary to your Cargo binary path (~/.cargo/bin)
cargo install --path crates/cli
```

Verify the installation:

```bash
rune-kit --version
# Output: rune-kit <version>
```

---

## CLI Reference

```text
Usage: rune-kit <COMMAND>

Commands:
  serve        Start the local MCP HTTP server and attached TUI
  install      Install an artifact from registry, URL, or local path
  uninstall    Uninstall an installed artifact (plugin or skill)
  list         List installed artifacts (plugins and skills)
  available    List available artifacts in the remote registry
  search       Search artifacts in the registry by keyword across names and descriptions
  update       Update installed artifacts from the registry or inspect update availability
  self-update  Self-update the rune-kit CLI binary to the latest release
  help         Print this message or the help of the given subcommand(s)
```

### 1. `rune-kit serve`

Start a persistent server at **http://127.0.0.1:47601/mcp**. With an interactive
terminal, the existing Rust TUI attaches to this server's live state. Non-interactive
launches run without terminal control sequences; use `--headless` explicitly in CI.

```bash
rune-kit serve --read-only
rune-kit serve git --headless --read-only
rune-kit serve ./dist/my-plugin.wasm --headless --port 47602
rune-kit serve --read-only -p allowed_dir=/absolute/workspace
```

By default all installed plugins are loaded; an optional name/artifact selects
one. `--all` is an explicit synonym for the default and cannot accompany a plugin.
`--allow-tool name__operation` is a server permission ceiling. Clients may narrow
that ceiling through request metadata but cannot change configuration or roots.
`--timeout` defaults to 30 seconds. Only loopback `--bind` addresses are accepted.
Set `RUNE_KIT_HTTP_TOKEN` to require a local bearer token; no token is printed.
Press q/Esc/Ctrl+C in the dashboard, or Ctrl+C headlessly, for graceful shutdown.
Occupied ports and broken installations fail with actionable errors.

**Breaking migration (first planned HTTP release: 0.6.0):** `rune-kit run` no longer
serves public stdio. It exits with HTTP migration guidance. Studio must connect
to the endpoint, not spawn a subprocess. Existing native plugin pipes remain
private and unchanged. See [HTTP contract and fixtures](docs/MCP-CONTRACT.md).

#### Environment Variable Parameter Injection

In addition to `-p KEY=VALUE`, `rune-kit serve` automatically inspects the host environment and forwards parameters into the plugin context:
1. **Prefixed Variables:** Any variable starting with `RUNE_PARAM_<NAME>` has the prefix stripped and the remaining key lowercased (e.g. `RUNE_PARAM_API_KEY=xyz` becomes `api_key=xyz`).
2. **Allowlisted System Variables:** The following host environment variables are forwarded automatically (lowercased): `ALLOWED_DIR`, `OUTPUT_DIR`, `OUTPUT_DIRECTORY`, `COOKIES_DIR`, `COOKIES_FILE`, `DEFAULT_TIMEZONE`, `REPO_PATH`, `MEMORY_FILE`, `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, `NO_PROXY`.

---

### 2. `rune-kit install`

Installs a plugin or skill from the remote registry, an HTTP(S) URL, or a local file path.

```bash
# Install a plugin from the registry (defaulting to WASM, falls back if needed)
rune-kit install git

# Prefer native companion/standalone binary if available
rune-kit install browser --native

# Pin a specific version
rune-kit install git --version 0.1.2

# Install from a local file (.wasm, .zip, .tar.gz, .exe)
rune-kit install ./dist/rune_memory.wasm

# Install a declarative skill (prompts/instructions) from file or markdown bundle
rune-kit install ./skills/code-review.md
rune-kit install #prompt-engineering
```

#### Flags & Options

| Flag | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `<TARGET>` | `String` (positional) | *Required* | Registry name, HTTP/HTTPS URL, or local file path to install. |
| `-v`, `--version <VERSION>` | `String` | `None` (latest) | Specific release version to install. |
| `--native` | `bool` | `false` | Prefer installing native binary if available in the registry. |
| `-h`, `--help` | `bool` | `false` | Print help information. |

#### Artifact Differentiation (Plugins vs. Skills)

`rune-kit install` automatically routes targets to the appropriate subsystem:
- **Skill Detection:** If `<TARGET>` ends with `.md`, `.txt`, `.rst`, `.markdown`, contains double underscores (`__`), starts with `#`, or matches an entry with `type: "skill"` in the registry, it is installed by `SkillManager` into `skills/<name>/`.
- **Plugin Detection:** All other targets route to `PackageManager` into `plugins/`. Any companion native binaries (`rune-<name>-native` or `<name>-native`) and `plugin.toml` files in the same directory are bundled alongside it.

---

### 3. `rune-kit uninstall`

Removes an installed plugin or skill and purges its local binaries and lockfile entries.

```bash
rune-kit uninstall git
rune-kit uninstall code-review
```

| Flag | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `<NAME>` | `String` (positional) | *Required* | Name of the installed plugin or skill to remove. |
| `-h`, `--help` | `bool` | `false` | Print help information. |

---

### 4. `rune-kit list`

Displays all installed plugins and skills in formatted terminal tables, including version, build types (`wasm`, `native`, or `wasm, native`), and sources.

```bash
rune-kit list
```

---

### 5. `rune-kit available`

Queries the remote registry and lists all available plugins and skills, displaying current local installed version vs. remote latest version.

```bash
rune-kit available
```

---

### 6. `rune-kit search`

Performs a case-insensitive search across names, descriptions, and tags in the remote registry.

```bash
rune-kit search git
rune-kit search prompt
```

| Flag | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `<KEYWORD>` | `String` (positional) | *Required* | Search keyword to match against artifact name, description, or tags. |
| `-h`, `--help` | `bool` | `false` | Print help information. |

---

### 7. `rune-kit update` (alias: `outdated`)

Inspects or updates installed plugins and skills against the remote registry.

```bash
# Check for updates without downloading
rune-kit update --check

# Update all outdated artifacts to the latest version
rune-kit update

# Update only specific artifacts
rune-kit update git memory
```

| Flag | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `[NAMES]...` | `Vec<String>` (positional) | `[]` (all) | Optional list of specific artifact names to inspect or update. |
| `-c`, `--check` | `bool` | `false` | Check for available updates and render the status table without downloading. |
| `-h`, `--help` | `bool` | `false` | Print help information. |

---

### 8. `rune-kit self-update`

Checks for and updates the running `rune-kit` CLI binary directly from GitHub releases matching the current OS target triple.

```bash
# Inspect whether a newer CLI binary exists
rune-kit self-update --check

# Perform the update
rune-kit self-update

# Force reinstall or pin a specific version
rune-kit self-update --version 0.2.1 --force
```

| Flag | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `-c`, `--check` | `bool` | `false` | Check if an update is available without downloading. |
| `-v`, `--version <VERSION>` | `String` | `None` (latest) | Specific release version to target (e.g. `0.2.0`). |
| `-f`, `--force` | `bool` | `false` | Reinstall even if the current binary is already up to date. |
| `-h`, `--help` | `bool` | `false` | Print help information. |

---

## File System & Storage Layout

Rune Kit stores managed data under `~/.rune/tools` on every platform. An absolute `RUNE_KIT_DATA_DIR` overrides this location. The default executable directory is `~/.rune`, shared with Infer. Legacy platform data is copied on first use; originals remain intact and conflicts fail clearly.

### Directory Structure

```text
<user-home>/.rune/tools/
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

Load WASM/native instances for embedded dispatch. Use `rune-kit serve` for the
supported public HTTP transport; this library example does not start a server:

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

    // One embedded dispatch, not a public stdio serving loop.
    let request = r#"{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#;
    let discovery = router.handle_jsonrpc(request).ok_or("No discovery response")?;
    println!("{discovery}");
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

Managed storage defaults to `~/.rune/tools` (plugins in `plugins/`). Set an absolute `RUNE_KIT_DATA_DIR` to override it. Legacy platform data is copied on first use, with originals retained; conflicts stop migration. See [MCP contract](docs/MCP-CONTRACT.md) for the new request format.


## Terminal inventory

Run `rune-kit serve` for the attached HTTP server dashboard (endpoint, uptime,
loaded/discovered plugins, active/recent requests and restrictions). Use arrows,
Tab to switch calls/tools, d to discover, i for configuration, r to refresh, and q to stop the server. See [dashboard controls](docs/TUI.md).

## Releases and contributor tooling

Successful develop CI prepares one reviewed release/update-version PR. The owner merges it into develop, then promotes develop to main with a merge commit or fast-forward. Main automation publishes the prepared version and exact changelog notes; release publication triggers platform artifacts. Merging into develop alone does not publish. Agents leave changes unstaged and uncommitted. Rust-only preparation previews and the required token/settings are documented in [the release flow](docs/RELEASING.md).

Hidden installation defaults: the executable is `<user-home>/.rune/rune-kit[.exe]`
and managed plugins/state are `<user-home>/.rune/tools`. On first ordinary Kit
command, known managed entries from `~/rune/tools` and older platform Kit storage
are copied with verification. Originals remain; divergent destination data stops
migration without overwrite. A completed visible-root migration takes precedence
over its retained platform backup. Explicit absolute `RUNE_KIT_DATA_DIR` skips
default migration. Installers retain old Kit/Infer binaries and Infer directories;
remove old Kit copies only after validating the new installation. Windows PATH
places the new root first while retaining the old root for Infer. POSIX adds a
guarded profile entry. `self-update` verifies the archive checksum and replaces
only the invoked executable, including custom installation locations.
