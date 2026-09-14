## 1. Overview

- **What it is:** a Rust Cargo workspace that implements an MCP (Model Context
  Protocol) plugin runtime plus a management CLI.
- **Stack:** Rust, edition 2024, workspace `resolver = 2`.
- **Entry points:**
  - `rune-kit-core` — library crate (the runtime). No `[[bin]]`.
  - `rune-kit-cli` — binary crate, `[[bin]] name = "rune"` → `src/main.rs`.
- **Dependency direction is one-way:** `rune-kit-cli` → `rune-kit-core`.
  `rune-kit-core` never depends back on the CLI.

## 2. Architecture Map

Two crates, one of which (core) hosts **two plugin execution models**, both
implemented in `crates/rune-kit-core/src/runtime.rs`:

1. **WASM plugin** — Extism/Wasmtime sandbox.
   - `WasmPluginInstance` struct (`runtime.rs:96`), `load_from_bytes` (`:109`),
     `Manifest::new([Wasm::data(bytes)])` (`:114`),
     `PluginBuilder::new(manifest).with_wasi(true).with_function("host_cmd_exec", …)` (`:143`).
2. **Native sidecar** — spawned as an OS child process, JSON-RPC over stdin/stdout.
   - `NativeSidecar` struct (`runtime.rs:200`), `spawn_process()` (`:237`),
     `Command::new(binary_path)` + `cmd.spawn()` (`:238`/`:244`), `send_request` (`~:266`).

**Dispatch:** a `PluginInstance` enum (`Wasm | Native`, `runtime.rs:415`) with
`load_wasm_from_file` (`:421`) / `load_native_from_file` (`:429`); per-call
dispatch in `get_info` / `list_tools` / `call_tool` (`:437–456`).

**All plugin interactions go through the `McpRouter` interface (spec, per maintainer).**
`McpRouter` (`crates/rune-kit-core/src/protocol.rs`) is the single JSON-RPC front
door: `handle_jsonrpc` routes `tools/list` / `tools/call` to the registered
`PluginInstance`s, and `run_stdio` serves the stdin/stdout loop. The CLI's
`run` command (`crates/rune-kit-cli/src/main.rs`, `Commands::Run` arm) builds a
`McpRouter`, registers instances, and serves via `router.run_stdio()` — the only
plugin-serving path. This is the spec, not a description of the current code:
if any code path ever interacts with a plugin without going through `McpRouter`,
that is an implementation bug to fix — do **not** change the spec to match the
code.

**Choosing the model (rule of thumb, per maintainer):** if removing the WASM
layer wouldn't remove any real sandboxing benefit — because the dangerous work
already happens host-side of a `host_fn` — build a **native sidecar** instead.
Don't pay the WASM/`host_fn` serialization tax for a pass-through.

> The two-crate split and the dual execution model are the same confirmed fact
> recorded under two graph nodes (`arch:two-crate-structure`,
> `arch:two-crate-dual-execution`). They are presented together here.

## 3. Naming & Style Conventions

Only these two are confirmed. Do not assume other conventions (function/variable
naming, module layout, commit style, test style) — they are not yet verified.

- **Serde wire naming — default, no repo-wide enum rule.**
  Serde uses default naming; there is **no** blanket `snake_case`/`rename_all`
  convention on public enums. Explicit `#[serde(rename)]` / `rename_all` appear
  only where an external contract requires it:
  - `crates/rune-kit-core/src/manifest.rs:5` — `ExecutionKind`, `rename_all = "snake_case"`.
  - `crates/rune-kit-core/src/manifest.rs:51` — `ToolDefinition.input_schema`,
    `rename = "inputSchema"` (alias `input_schema`).
- **Error types are `thiserror` enums.**
  `#[derive(Error, Debug)]`, a human-readable `#[error(…)]` per variant, and
  `#[from]` for automatic conversion:
  - `crates/rune-kit-core/src/package.rs:13–27` — `PackageError`.
  - `crates/rune-kit-core/src/runtime.rs:11–23` — `RuntimeError`.
  - *Scope:* this applies to `rune-kit-core` only. The CLI deliberately does not
    use thiserror (see below).
- **CLI errors are boxed, not `thiserror`.**
  `main` returns `Result<(), Box<dyn std::error::Error>>`
  (`crates/rune-kit-cli/src/main.rs:12`); core `PackageError`/`RuntimeError`
  propagate via `?` and a propagated `Err` yields Rust's default non-zero exit
  (1). No CLI-local error enum exists.

## 4. Key Flows

### Native sidecar lifecycle — "fail fast at load, self-heal on crash"

The native sidecar child process has **two** spawn paths in `runtime.rs`
(intentional design, per maintainer):

- **Eager spawn** in `new` (`runtime.rs:233`) — surfaces load errors immediately.
- **Lazy respawn** in `send_request` (`runtime.rs:267`) — restarts the child if it
  died between calls.

### Native sidecar request/response — one-in-flight (see Known Limitations)

`send_request` (`runtime.rs:316–320`) returns the **first** JSON-object line read
from stdout and does **not** correlate on the JSON-RPC `id` field.

## 5. Known Inconsistencies / Exceptions

- **Native sidecar is strictly one-request-at-a-time (a limitation, NOT intentional).**
  `send_request` (`runtime.rs:316–320`) ignores the JSON-RPC `id`, so the sidecar
  cannot interleave responses, and any unsolicited notification would be consumed
  as the pending response. Do not "fix" the double-spawn in §4 — that half is
  intentional; the one-in-flight behavior is the actual constraint.
- **`rune` self-update exits 0 even on failure (intentional, per maintainer).**
  The `Commands::SelfUpdate` arm in `crates/rune-kit-cli/src/main.rs` matches
  the error, `eprintln!`s it, and falls through to `Ok(())` — so a failed
  self-update exits 0, unlike every other failing command (exit 1). Self-update
  is best-effort and must not fail the process.

## 6. Open Questions (not yet confirmed — do not encode as conventions)

- **WASM sandbox: default network policy** — appears to allow-all when no
  `allowed_hosts` is set. Intentional or a gap?
- **WASM sandbox: `host_cmd_exec`** — host-command escape hatch exposed to plugins.
  Intentional capability or over-broad?
- **WASM sandbox: `printer_ip` special-case.** Purpose / intent unclear.
- **Graph hygiene:** `arch:two-crate-dual-execution` and `arch:two-crate-structure`
  overlap; candidate to merge into one node.

---
*Generated from confirmed graph facts, last updated 2026-09-15. Update via diff-patch as more
facts are confirmed; do not regenerate sections that are already correct.*
