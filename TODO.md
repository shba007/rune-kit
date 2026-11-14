# Rune Kit — Feature Todo

## 1. Core Architecture & Transport
- [x] High-performance single-crate runtime and CLI binary (`rune-kit`) in Rust (Edition 2024)
- [x] Model Context Protocol (MCP) Streamable HTTP transport (`/mcp`, protocol specification `2026-07-28`)
- [x] Ephemeral port binding (`--port 0`) with bound port reporting on stderr
- [x] Unified MCP multiplexer (`McpRouter`) for tools (`<namespace>__<tool>`), resources (`rune://<namespace>/<uri>`), and prompts (`<namespace>__<prompt>`)
- [x] JSON-RPC request-scoped SSE streaming response formatting (`event: message`) and early JSON error handling
- [x] Standard mirrored header validation (`Mcp-Method`, `Mcp-Name`, `MCP-Protocol-Version`, UTF-8 Base64 sentinels)
- [x] Graceful server teardown on `Ctrl+C`, `q`, `Esc`, and Unix `SIGTERM` signals
- [ ] Multi-tenant client authentication and per-token permission scopes
- [ ] Resumable MCP session streams and server-initiated push notification channels

## 2. WebAssembly Plugin Sandbox
- [x] In-process Wasmtime / Extism virtual machine targeting `wasm32-wasip1`
- [x] Memory isolation and sandboxed execution for pure compute plugins
- [x] Declarative capability gating in `plugin.toml` (scoped filesystem access `allowed_dir`, network egress `allowed_hosts`, command execution allowlists)
- [x] Host function bridge:
  - [x] `host_cmd_exec` for invoking allowlisted external utilities
  - [x] `host_sidecar_exec` for bridging WASM guest calls to companion native processes
  - [x] `host_get_binary_path` for querying host-provisioned static CLI tools
- [x] Workspace filesystem isolation flag (`--workspace-only`) restricting WASM plugins to dedicated folders
- [ ] Configurable default network policy when `allowed_hosts` is omitted (enforce explicit deny vs permissive)
- [ ] WASM Component Model (`wasm32-wasip2`) runtime target support

## 3. Direct Native Process Supervision
- [x] Standalone native executable execution (`ExecutionKind::Native`) via newline-delimited JSON-RPC stdio
- [x] WASM companion native sidecar delegation (`rune-<name>-native` / `<name>-native`)
- [x] Eager executable existence and permission validation (Unix 0755)
- [x] Lazy spawning on first request with transparent auto-respawn on process crash
- [x] Clean child process termination on server teardown
- [x] OS-specific native process launch boundary and isolation checks (`native_isolation.rs`)
- [ ] Complete cross-platform validation gate (`VALIDATED_ON_ALL_PLATFORMS = true`) across Linux, macOS, and Windows
- [ ] Full Windows job object assignment for untracked native child process trees
- [ ] Complete migration of printer IP routing logic from `runtime.rs` to native sidecar implementation
- [ ] Multi-threaded concurrent dispatch over native process stdio (currently serialized sequentially per process)

## 4. Host-Managed External Binary Engine
- [x] Automatic HTTPS download of pre-pinned static builds (`ffmpeg`, `yt-dlp`, `gallery-dl`, `git`)
- [x] Architecture- and OS-aware binary selection (`std::env::consts::{OS, ARCH}`)
- [x] Mandatory SHA-256 integrity verification before archive extraction
- [x] Pure-Rust archive decompression (`.zip`, `.tar.gz`) without host system tools
- [x] Atomic directory activation using `.verified` markers in shared cache (`~/.rune/tools/_bin/`)
- [x] Cross-process file locking (`.lock`) preventing duplicate simultaneous downloads
- [x] Runtime availability query API (`host_get_binary_path`) returning verified paths or download progress status
- [ ] Automatic update and version pruning for cached external binary tools

## 5. Artifact & Package Management
- [x] Two-tier artifact management: executable plugins (`installed.json`) and content skills (`installed_skills.json`)
- [x] Automated artifact kind detection (plugins vs declarative prompt/markdown skills)
- [x] Package management CLI commands:
  - [x] `rune-kit install <target>` (supports registry names, local files, URLs, and git repos)
  - [x] `rune-kit uninstall <name>`
  - [x] `rune-kit list` (displays installed plugins and skills with execution kinds and versions)
  - [x] `rune-kit available` (queries remote registry catalog)
  - [x] `rune-kit search <keyword>` (fuzzy search across names and descriptions)
  - [x] `rune-kit update` / `outdated` (checks and updates installed artifacts)
- [x] Staged changes management (`rune-kit changes` list, apply, backups, restore) for safe workspace edits
- [x] Third-party Streamable HTTP MCP server proxying and management (`rune-kit remote` add, list, remove)
- [x] First-run interactive setup CLI (`rune-kit setup`)
- [x] Standalone executable self-update (`rune-kit self-update`) with checksum validation and `--check` mode

## 6. Terminal Dashboard & Monitoring
- [x] Attached Ratatui TUI dashboard for interactive `rune-kit serve` sessions
- [x] Headless mode (`--headless` or non-TTY detection) for background and CI services
- [x] Real-time request log with bounded history (64 entries), outcome status, and duration
- [x] Plugin inventory list with execution mode (WASM vs Native) and active version
- [x] Configuration inspection view (press `i`) showing bound paths, allowed roots, and limits
- [x] Terminal restoration guard on normal and error exits
- [x] Full-frame `NO_COLOR` and `RUNE_ASCII=1` theme support
- [ ] Background daemon/systemd/Windows Service installation command (`rune-kit service install`)
