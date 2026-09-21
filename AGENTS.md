# Rune Kit - Agent Documentation

## 1. Overview

- **What it is:** A Rust Cargo workspace implementing a high-performance Model Context Protocol (MCP) plugin runtime host and package manager CLI (`rune`).
- **Stack:** Rust, edition 2024, workspace `resolver = 2`.
- **Artifact Model:** `rune-kit` manages two distinct artifact classes:
  - **Plugins (Code):** Executable MCP services delivering tools, resources, and prompts to MCP clients. Plugins run in one of two execution tiers:
    1. **WebAssembly (`ExecutionKind::Wasm`):** Sandboxed guest modules (`.wasm`) compiled to `wasm32-wasip1`, running in Extism/Wasmtime with optional companion native sidecars.
    2. **Direct Standalone Native (`ExecutionKind::Native`):** Compiled OS binaries (`.exe` or ELF/Mach-O) communicating via JSON-RPC stdio, utilized when operations are fundamentally incompatible with WASM virtualization.
  - **Skills (Content):** Declarative prompts, context instructions, and reference content packages (e.g., `SKILL.md`) installed to local storage without execution privileges.
- **Core Architectural Premise:**
  - **WASM-First by Preference:** WebAssembly (`wasm32-wasip1`) is the default sandboxing model for portable, secure, and cross-platform compute.
  - **Direct Standalone Native Support:** When plugins require raw system capabilities that cannot run within WASM (e.g., low-level OS event loops, desktop/window automation, proprietary device drivers, native graphics/display APIs, or complex multithreading), `rune-kit` allows direct execution of native companion binaries as first-class MCP servers.
  - **WASM-Mediated Native Sidecars:** For hybrid workloads, WASM guest modules maintain the MCP interface while delegating heavy OS routines to a companion native process (`rune-<name>-native`) via host-mediated execution bridges.
  - **Host-Managed External Binaries:** External CLI dependencies (e.g., `ffmpeg`, `yt-dlp`, `gallery-dl`) are downloaded, verified, cached, and managed by **`rune-kit`**, not by `rune-tools`. `rune-kit` provisions verified static binaries on clean user machines and exposes resolved executable paths to the plugin runtime.
- **Entry Points:**
  - `rune-kit-core` — Library crate (the runtime). Contains no `[[bin]]`.
  - `rune-kit-cli` — Both a binary crate (`[[bin]] name = "rune"` → `src/main.rs`) and a library crate (`[lib] name = "rune_kit_cli"` → `src/lib.rs`). All CLI command dispatch and table rendering live in `lib.rs`; `main.rs` is a wrapper invoking `rune_kit_cli::main()` and propagating non-zero exit codes on failure.
- **Dependency Direction is One-Way:** `rune-kit-cli` → `rune-kit-core`. `rune-kit-core` never depends on the CLI.

---

## 2. Architecture Map

```text
┌──────────────────────────────────────────────────────────────┐
│                    rune-kit-cli ("rune")                     │
└──────────────────────────────┬───────────────────────────────┘
                               │
                               ▼
┌──────────────────────────────────────────────────────────────┐
│                       rune-kit-core                          │
│                                                              │
│  ┌────────────────────────────────────────────────────────┐  │
│  │             McpRouter (protocol.rs)                    │  │
│  │  • Single JSON-RPC stdio transport front door          │  │
│  │  • Unified dispatch for tools, resources, and prompts  │  │
│  └──────────────┬───────────────────────────┬─────────────┘  │
│                 │                           │                │
│                 ▼                           ▼                │
│  ┌────────────────────────────┐ ┌─────────────────────────┐  │
│  │     WasmPluginInstance     │ │      NativeSidecar      │  │
│  │  • Extism/Wasmtime sandbox │ │  (Standalone Execution) │  │
│  │  • Capabilities gate       │ │  • Direct stdio MCP     │  │
│  └──────────────┬─────────────┘ │  • Deep OS/C-FFI access │  │
│                 │ Host Bridge   └─────────────────────────┘  │
│                 ▼                                            │
│  ┌────────────────────────────┐                              │
│  │ Companion Native Sidecar   │                              │
│  │ (rune-<name>-native)       │                              │
│  │ • WASM-mediated execution  │                              │
│  └────────────────────────────┘                              │
│                 │ Host FFI                                   │
│                 ▼                                            │
│  ┌────────────────────────────┐                              │
│  │ External Binary Engine     │                              │
│  │ (_bin/<name>/<version>/)   │                              │
│  │ • ffmpeg, yt-dlp, etc.     │                              │
│  └────────────────────────────┘                              │
└──────────────────────────────────────────────────────────────┘
```

### 2.1 The Execution Architecture

`rune-kit-core` hosts the plugin runtime across two execution models managed under `PluginInstance` (`crates/rune-kit-core/src/runtime.rs`):

```rust
pub enum PluginInstance {
    Wasm(WasmPluginInstance),
    Native(NativeSidecar),
}
```

1. **WASM Plugin Sandbox (`WasmPluginInstance`):**
   - Extism/Wasmtime sandbox targeting `wasm32-wasip1`.
   - Initialized via `Manifest::new([Wasm::data(bytes)])` and `PluginBuilder::new(manifest).with_wasi(true)`.
   - Exposes typed host functions to the guest:
     - `host_cmd_exec` — Invokes allowlisted external CLI utilities.
     - `host_sidecar_exec` — Bridges WASM requests to companion native sidecars (`rune-<name>-native`).
     - `host_get_binary_path` — Resolves paths to host-provisioned static tools (`ffmpeg`, `yt-dlp`).
   - Restricts network and filesystem access according to declared manifest capabilities.

2. **Native Process Engine (`NativeSidecar`):**
   - Manages companion or standalone OS processes communicating via stdio JSON-RPC.
   - **Standalone Plugin Mode:** Directly bound to `McpRouter` when a plugin declares `execution_kind = "native"` or is installed directly as a native executable.
   - **Companion Sidecar Mode:** Supervised companion process (`rune-<name>-native`) called internally by a WASM guest module through `host_sidecar_exec`.
   - Lifecycle management handles piped stdio, lazy respawn on process exit, and clean SIGTERM/kill handling on teardown.

### 2.2 Dispatch via `McpRouter`

All external client MCP interactions route through `McpRouter` (`crates/rune-kit-core/src/protocol.rs`):
- Single JSON-RPC stdio transport front door for clients.
- Dispatches across registered plugin instances (`PluginInstance::Wasm` and `PluginInstance::Native`) for all six core MCP methods:
  - `tools/list` & `tools/call` (namespaced via `<namespace>__<tool_name>`)
  - `resources/list` & `resources/read` (namespaced via `rune://<namespace>/<plugin-local-uri>`)
  - `prompts/list` & `prompts/get` (namespaced via `<namespace>__<prompt_name>`)
- Clients interact with a unified interface regardless of whether a plugin is executed in WASM or directly as a native process.

### 2.3 Choosing the Execution Model

| Model | Implementation | When to Use |
| :--- | :--- | :--- |
| **Pure WASM** | `WasmPluginInstance` | Standard API integrations, deterministic compute, document parsers, transforms, and sandboxed file/network operations. |
| **WASM + Native Sidecar** | `WasmPluginInstance` + companion `NativeSidecar` | Scenarios where the MCP contract is maintained in portable WASM, but performance-heavy native workloads or OS printing (CUPS/WinSpool) are delegated via host bridges. |
| **Direct Standalone Native** | `NativeSidecar` (direct) | Low-level OS automation, window manager interaction, native GUI automation, hardware/driver access, or existing native MCP binaries that cannot be cross-compiled to WASM. |

---

## 3. External Binary Provisioning Engine

`rune-kit-core` ensures that third-party CLI dependencies (`ffmpeg`, `yt-dlp`, etc.) are installed, verified, and accessible without requiring external package managers (Homebrew, apt, cargo, pip, or npm).

### 3.1 Manifest Contract (`plugin.toml`)

Plugin requirements are declared in `plugin.toml` and parsed by `crates/rune-kit-core/src/manifest.rs`:

```toml
[capabilities]
network_hosts = ["api.example.com"]
filesystem = { mode = "scoped", root_param = "allowed_dir" }
exec = { allowed_binaries = ["ffmpeg", "yt-dlp"] }

[dependencies.binaries.ffmpeg]
version = ">=6.1"
optional = false
description = "Audio extraction and media transcoding"

[dependencies.binaries.yt-dlp]
version = ">=2024.01.01"
optional = true
description = "Stream extraction"
```

### 3.2 Provisioning Lifecycle & Storage

1. **Storage Cache:** External tools are maintained in a shared, host-managed cache:
   ```text
   dirs::data_dir()/rune-kit/_bin/
   ├── ffmpeg/
   │   └── 6.1.1/
   │       ├── ffmpeg (executable, mode 0755)
   │       ├── ffprobe
   │       └── .verified (SHA-256 validation marker)
   └── yt-dlp/
       └── 2024.03.10/
           ├── yt-dlp
           └── .verified
   ```
2. **Download & Verification:**
   - Pre-pinned static builds mapped to `std::env::consts::{OS, ARCH}`.
   - Downloaded via HTTPS to temporary `.partial-*` directories.
   - Mandatory SHA-256 validation prior to unpacking.
   - Pure-Rust archive extraction (`zip`, `tar.gz`) without host system utilities.
   - Target version directories are activated atomically upon writing `.verified`.
3. **Cross-Process Concurrency:** File locks (`_bin/<name>/.lock`) guard against redundant concurrent downloads when multiple plugins initialize concurrently.

### 3.3 Runtime Query & Resolution API

The host exposes tool availability to plugins via `host_get_binary_path(name)`:
- Verifies `name` against `capabilities.exec.allowed_binaries`.
- Returns the absolute path when installed and verified.
- Returns non-fatal status `InProgress { progress_percent, eta_seconds }` if currently downloading.
- Returns `NotFound` if unmanaged or missing.
- Provides actionable errors for MCP clients:
  `{"status": "error", "error": "ffmpeg is currently being provisioned (45% downloaded). Please retry in 30 seconds."}`

---

## 4. Skills Management Subsystem (Content Artifacts)

In addition to executable plugins, `rune-kit` natively manages **Skills**—reusable prompt packages, instructions, context documents, and reference materials.

### 4.1 Plugins vs. Skills Architectural Boundary

| Dimension | Plugins (`PackageManager`) | Skills (`SkillManager`) |
| :--- | :--- | :--- |
| **Nature** | Executable MCP servers (WASM or Native) | Declarative markdown, prompts, instructions |
| **Execution** | Extism WASM runtime / OS process | Non-executable static content |
| **Storage** | `dirs::data_dir()/rune-kit/plugins/` | `dirs::data_dir()/rune-kit/skills/<name>/` |
| **Lockfile** | `installed.json` (`Lockfile`) | `installed_skills.json` (`SkillLockfile`) |
| **Registry Schema** | `RegistryPluginSummary` | `RegistrySkillSummary` (includes `author`, `tags`) |

### 4.2 Target Detection & Installation

The CLI automatically differentiates between skills and plugins during `rune install <target>`:
- **Skill Detection:** Targets ending in `.md`, `.txt`, `.rst`, `.markdown`, containing double underscores (`__`), or beginning with `#` route to `SkillManager::install_skill`.
- **Packaging:** Skills may be installed as raw standalone text files or compressed bundles (`.zip`, `.tar.gz`).
- **Integrity Tracking:** constituent files are extracted into `skills/<name>/` and hashed (`SkillFile.sha256`) to maintain lockfile verification.

---

## 5. Naming & Style Conventions

- **Serde Wire Naming:**
  - `crates/rune-kit-core/src/manifest.rs`:
    - `ExecutionKind`: `rename_all = "snake_case"` (`"wasm"`, `"native"`).
    - `ToolDefinition.input_schema`: `rename = "inputSchema"`, aliased to `input_schema`.
    - `ResourceDefinition.mime_type`: `rename = "mimeType"`, aliased to `mime_type`.
  - `PromptDefinition` and `PromptArgument` match MCP wire schemas directly without explicit renames.
  - `BinaryDependencySpec` deserializes `[dependencies.binaries]` using snake_case properties.
- **Error Types (`thiserror`):**
  - All core error types are strongly-typed enums using `#[derive(Error, Debug)]`:
    - `crates/rune-kit-core/src/package.rs` — `PackageError`.
    - `crates/rune-kit-core/src/runtime.rs` — `RuntimeError`.
    - `crates/rune-kit-core/src/provision.rs` — `ProvisionError`.
  - CLI errors remain boxed (`Result<(), Box<dyn std::error::Error>>`) and print to `eprintln!` before terminating with `std::process::exit(1)`.
- **Output Rendering:**
  - `crates/rune-kit-cli/src/output.rs` provides `Table` and `Column` definitions (supporting fixed and flexible widths with char-aware truncation). Header rows are verified via integration tests in `crates/rune-kit-cli/tests/output_tests.rs`.
  - The `SkillRow` trait abstracts fields across `InstalledSkill` and `RegistrySkillSummary` for skill table rendering.

---

## 6. Key Flows

### 6.1 Plugin Installation and Registration

During `rune install <target>`:
1. **Target Inspection:**
   - **Local `.wasm`:** Installed as `ExecutionKind::Wasm`. Companion sidecars (`rune-<name>-native` or `<name>-native`) and `plugin.toml` files in the same directory are automatically packaged alongside it.
   - **Local Native Binary / Archive (`.exe`, ELF, `.zip`, `.tar.gz`):** Installed as `ExecutionKind::Native` when native OS capabilities are required.
   - **Remote Registry (`name`):** Fetches the index entry, downloading WASM, native, or hybrid artifacts based on target triple matching.
2. **Dependency Audit:** Evaluates `manifest.dependencies.binaries`. Non-optional dependencies are downloaded and verified via `BinaryProvisioner`.
3. **Registration:** Records binary paths, execution kind, SHA-256 hash, and parameters to `installed.json`.

### 6.2 MCP Request Execution Flow

1. The client issues a JSON-RPC request over stdio (`tools/*`, `resources/*`, or `prompts/*`).
2. `McpRouter` decodes the namespace prefix and routes the call to the corresponding `PluginInstance`:
   - **WASM Route:** Invokes guest exports (`mcp_call_tool`, `mcp_read_resource`, `mcp_get_prompt`). If native work is required, WASM dispatches to the companion binary via `host_sidecar_exec`.
   - **Native Route:** Sends JSON-RPC commands directly over stdio pipes to the running standalone `NativeSidecar` process and reads output lines.
3. Formatted JSON-RPC responses are returned to the client through stdout.

### 6.3 Native Process Lifecycle (Companion and Standalone)

Whether operating as a WASM companion or as a standalone native server:
- **Eager Check:** Confirms the binary exists and has executable permissions (0755 on Unix) during registration.
- **Lazy Spawn & Respawn:** Processes spawn on first invocation. If the native process exits or crashes between calls, it restarts transparently.
- **Controlled Teardown:** The `Drop` implementation kills child processes cleanly and cleans up associated stdio pipes.

### 6.4 Skill Installation and Lifecycle

1. `rune install <target>` identifies the artifact as content.
2. `SkillManager` downloads or copies files into `skills/<name>/`.
3. Member files are hashed (SHA-256) and logged in `installed_skills.json`.
4. Installed skills are listed with `rune list` and updated with `rune update`.

---

## 7. Known Inconsistencies / Exceptions

- **`rune self-update` Exits 0 on Failure (Intentional):**
  The `Commands::SelfUpdate` handler in `lib.rs` logs errors to `eprintln!` and returns `Ok(())` to prevent automated background updates from failing parent processes.
- **Sidecar JSON-RPC Serialization Constraint:**
  Native sidecar and standalone processes communicate via single-threaded stdio pipes per plugin instance. Calls are serialized sequentially per process.

---

## 8. Open Questions

- **WASM Sandbox Default Network Policy:** Determine whether omitting `allowed_hosts` in `plugin.toml` should deny network access by default or maintain permissive network defaults.
- **Printer IP Routing Migration:** Evaluate whether legacy host printer routing logic in `runtime.rs` should be completely delegated to `rune-print`'s native implementation.

---

*Generated from confirmed architectural contracts and repository implementations.*  
*Last updated: 2026-09-21.*
