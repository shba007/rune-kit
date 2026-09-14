# Rune Kit — MCP Runtime & Package Manager

Rune Kit is a Rust runtime that loads, manages, and serves MCP (Model Context
Protocol) plugins. It ships as a single `rune` CLI binary that acts both as a
package manager for MCP plugins and as an MCP server gateway over stdio.

Plugins run in one of two execution models:

- **WASM** — sandboxed via Extism/Wasmtime (`.wasm` artifacts)
- **Native** — sidecar child process speaking JSON-RPC over stdin/stdout

## Workspace layout

| Crate | Type | Purpose |
|---|---|---|
| `rune-kit-core` | library | Runtime: plugin loading, `McpRouter` protocol layer, package manager |
| `rune-kit-cli` | binary (`rune`) | CLI: install / manage plugins, serve MCP over stdio |

## Getting started

### Build

```bash
cargo build --release
```

### Install the CLI

```bash
cargo install --path crates/rune-kit-cli
```

This produces the `rune` binary.

## Quick start

```bash
# Install a plugin by name from the registry, or from a local file
rune install my-tool
rune install ./path/to/rune_my-tool.wasm

# See what's installed
rune list

# Serve it as an MCP server over stdio
rune run my-tool
```

## CLI reference

| Command | Description |
|---|---|
| `rune run [PLUGIN]` | Start the MCP server gateway over stdio. `PLUGIN` is an installed name or a local file path (`.wasm` → WASM, anything else → native sidecar). |
| `rune run --all` | Gateway mode: serve **all** installed plugins at once. |
| `rune install TARGET` | Install from registry name, URL, or local path. `--version`, `--native` (prefer native sidecar build). |
| `rune uninstall NAME` | Remove an installed plugin. |
| `rune list` | List installed plugins (name, version, kind, description, source). |
| `rune available` | List all plugins in the remote registry. |
| `rune search KEYWORD` | Search the registry by name or description. |
| `rune update [NAMES]` | Update installed plugins (alias: `outdated`). `--check` to inspect only, `--native` to prefer native builds. |
| `rune self-update` | Update the `rune` binary itself. `--check`, `--version`, `--force`. |

### Injecting parameters

`rune run` accepts `-p KEY=VALUE` pairs which are injected into the plugin
alongside the lowercased host environment:

```bash
rune run my-tool -p allowed_dir=/workspace
```

## Where things live

Installed plugins and the lockfile (`installed.json`) are stored under your
OS data directory: `~/.local/share/rune-kit` (Linux),
`~/Library/Application Support/rune-kit` (macOS),
`%APPDATA%\rune-kit` (Windows).

## The registry

The default registry index is served from the
[rune-tools](https://github.com/shba007/rune-tools) repository
(`registry/index.json`). Plugin builds (WASM and/or native) are distributed
from the same project — see
[Rune Tools](https://github.com/shba007/rune-tools) for building and
publishing plugins.

## License

MIT
