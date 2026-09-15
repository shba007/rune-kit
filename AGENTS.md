# Rune Kit - Agent Documentation

## 1. Overview

- **What it is:** a Rust Cargo workspace that implements an MCP (Model Context
  Protocol) plugin runtime plus a management CLI.
- **Stack:** Rust, edition 2024, workspace `resolver = 2`.
- **Entry points:**
  - `rune-kit-core` — library crate (the runtime). No `[[bin]]`.
  - `rune-kit-cli` — **both** a binary crate (`[[bin]] name = "rune"` →
    `src/main.rs`) **and** a library crate (`[lib] name = "rune_kit_cli"` →
    `src/lib.rs`) in the same `Cargo.toml`. This is new: all CLI command
    logic (`Commands` match arms, table rendering) lives in `lib.rs`;
    `main.rs` is a five-line wrapper that calls `rune_kit_cli::main()`. The
    split appears to exist to make CLI output testable — see §3's testing
    note — but see §5 for a real behavior change this split introduced.
- **Dependency direction is one-way:** `rune-kit-cli` → `rune-kit-core`.
  `rune-kit-core` never depends back on the CLI.

## 2. Architecture Map

Two crates, one of which (core) hosts **two plugin execution models**, both
implemented in `crates/rune-kit-core/src/runtime.rs`:

1. **WASM plugin** — Extism/Wasmtime sandbox.
   - `WasmPluginInstance` struct, `load_from_bytes`,
     `Manifest::new([Wasm::data(bytes)])`,
     `PluginBuilder::new(manifest).with_wasi(true).with_function("host_cmd_exec", …)`.
2. **Native sidecar** — spawned as an OS child process, JSON-RPC over stdin/stdout.
   - `NativeSidecar` struct, `spawn_process()`,
     `Command::new(binary_path)` + `cmd.spawn()`, `send_request`.

**Dispatch:** a `PluginInstance` enum (`Wasm | Native`) with
`load_wasm_from_file` / `load_native_from_file`; per-call dispatch now covers
**six** methods, not three: `get_info` / `list_tools` / `call_tool` /
`list_resources` / `read_resource` / `list_prompts` / `get_prompt`. (Line
numbers intentionally omitted here — the file has grown enough since the
three-method version that citing stale numbers would be worse than citing
none; re-grep before quoting a line reference.)

**All plugin interactions go through the `McpRouter` interface (spec, per maintainer).**
`McpRouter` (`crates/rune-kit-core/src/protocol.rs`) is the single JSON-RPC front
door. It now actually implements `resources/list`, `resources/read`,
`prompts/list`, and `prompts/get` — these were previously hardcoded to return
empty arrays; they now aggregate across registered `PluginInstance`s the same
way `tools/list`/`tools/call` always have. Resource URIs are namespaced as
`rune://<namespace>/<plugin-local-uri>`, parsed back apart on `resources/read`
by splitting on the first `/` after the scheme. Prompts use the same
`namespace__name` convention tools already use. `run_stdio` serves the
stdin/stdout loop. The CLI's `run` command (`crates/rune-kit-cli/src/lib.rs`,
`Commands::Run` arm — moved from `main.rs`, see §1) builds a `McpRouter`,
registers instances, and serves via `router.run_stdio()` — still the only
plugin-serving path. This is the spec, not a description of the current code:
if any code path ever interacts with a plugin without going through `McpRouter`,
that is an implementation bug to fix — do **not** change the spec to match the
code.

**`host_cmd_exec` gained a resolution fallback.** If `req.program` isn't
absolute and doesn't exist as given, it's now also checked against
`dirs::data_dir()/rune-kit/plugins/<program>` (and `<program>.exe`) before
failing — i.e. a WASM plugin's exec request can resolve against the
directory where installed native sidecar binaries live, not just `PATH`.
This widens `host_cmd_exec`'s reach; flagged in §6, not asserted as
intentional or a gap.

**Choosing the model (rule of thumb, per maintainer):** if removing the WASM
layer wouldn't remove any real sandboxing benefit — because the dangerous work
already happens host-side of a `host_fn` — build a **native sidecar** instead.
Don't pay the WASM/`host_fn` serialization tax for a pass-through.

> The two-crate split and the dual execution model are the same confirmed fact
> recorded under two graph nodes (`arch:two-crate-structure`,
> `arch:two-crate-dual-execution`). They are presented together here.

## 3. Naming & Style Conventions

Only these are confirmed. Do not assume other conventions (function/variable
naming, module layout, commit style) — they are not yet verified.

- **Serde wire naming — default, no repo-wide enum rule.**
  Serde uses default naming; there is **no** blanket `snake_case`/`rename_all`
  convention on public enums. Explicit `#[serde(rename)]` / `rename_all` appear
  only where an external contract requires it — now **three** confirmed
  instances, not two:
  - `crates/rune-kit-core/src/manifest.rs` — `ExecutionKind`, `rename_all = "snake_case"`.
  - `crates/rune-kit-core/src/manifest.rs` — `ToolDefinition.input_schema`,
    `rename = "inputSchema"` (alias `input_schema`).
  - `crates/rune-kit-core/src/manifest.rs` — `ResourceDefinition.mime_type`,
    `rename = "mimeType"` (alias `mime_type`) — same reasoning as
    `input_schema`: the MCP wire format is camelCase, the Rust field stays
    idiomatic snake_case, and the alias means either JSON casing
    deserializes cleanly.
  - `PromptDefinition`/`PromptArgument` need **no** renames — every field is
    already spelled the same both ways (`name`, `description`, `arguments`,
    `required`) — this is not an exception to the rule, it just never
    triggers the rule's condition.
- **Error types are `thiserror` enums.**
  `#[derive(Error, Debug)]`, a human-readable `#[error(…)]` per variant, and
  `#[from]` for automatic conversion:
  - `crates/rune-kit-core/src/package.rs` — `PackageError`.
  - `crates/rune-kit-core/src/runtime.rs` — `RuntimeError`.
  - *Scope:* this applies to `rune-kit-core` only. The CLI deliberately does not
    use thiserror (see below).
- **CLI errors are boxed, not `thiserror` — but exit-code propagation is now broken.**
  `rune_kit_cli::main` (now in `crates/rune-kit-cli/src/lib.rs`, not
  `main.rs` — see §1) still returns `Result<(), Box<dyn std::error::Error>>`,
  and core `PackageError`/`RuntimeError` still propagate into it via `?`. What
  changed: the actual binary entry point,
  `crates/rune-kit-cli/src/main.rs`, is now:
  ```rust
  #[tokio::main]
  async fn main() {
      library_main().await;
  }
  ```
  `main()` returns `()`, not the library's `Result` — so the `Result` is
  evaluated and discarded, not propagated to the process exit code. See §5:
  this is a new, likely-unintentional behavior change, not a continuation of
  the previously-documented pattern.
- **CLI output rendering has its own tested abstraction.**
  `crates/rune-kit-cli/src/output.rs` defines `Table`/`Column` (fixed vs.
  flexible-width columns, char-aware truncation only on flexible columns) and
  is covered two ways: inline `#[cfg(test)] mod tests` in `output.rs` itself,
  plus a separate integration test at `crates/rune-kit-cli/tests/output.rs`
  that asserts exact header-row strings for each table (`render_list_table`,
  `render_registry_table`, `render_update_status_table`). Treat header-row
  wording/column order as load-bearing — the tests will catch drift, so
  changing a header intentionally means updating the matching assertion in
  `tests/output.rs`, not silencing it.

## 4. Key Flows

### Native sidecar lifecycle — "fail fast at load, self-heal on crash"

The native sidecar child process has **two** spawn paths in `runtime.rs`
(intentional design, per maintainer):

- **Eager spawn** in `new` — surfaces load errors immediately.
- **Lazy respawn** in `send_request` — restarts the child if it died between
  calls.

This applies uniformly to all four call kinds now, not just tools: `list_tools`,
`call_tool`, `list_resources`, `read_resource`, `list_prompts`, and
`get_prompt` all funnel through the same `send_request`, so they all inherit
both the spawn behavior above and the one-in-flight limitation below equally.

### Native sidecar request/response — one-in-flight (see Known Limitations)

`send_request` returns the **first** JSON-object line read from stdout and
does **not** correlate on the JSON-RPC `id` field. Unchanged by the
resources/prompts additions — every new sidecar method reuses this same
function, so it inherits the same limitation without modification.

## 5. Known Inconsistencies / Exceptions

- **`rune` now exits 0 on every command failure, not just self-update
  (new — likely a regression, not confirmed intentional).** Previously, only
  `Commands::SelfUpdate` deliberately swallowed its error and exited 0
  (documented below, and that one *is* confirmed intentional). Since
  `main.rs` was split into a thin wrapper (§1/§3) that discards
  `library_main()`'s `Result` entirely, **every** command — `Install`,
  `Run`, `Update`, all of it — now exits 0 even when it fails, with no error
  printed to the terminal at all (not even the `eprintln!` the old self-update
  arm used). This is a strictly worse outcome than the previously-documented
  self-update-specific exception, and looks like an artifact of the lib/bin
  split rather than a deliberate choice extending "self-update is best-effort"
  to the entire CLI. Needs maintainer confirmation before treating either
  reading as settled.
- **Native sidecar is strictly one-request-at-a-time (a limitation, NOT intentional).**
  `send_request` ignores the JSON-RPC `id`, so the sidecar cannot interleave
  responses, and any unsolicited notification would be consumed as the
  pending response. Do not "fix" the double-spawn in §4 — that half is
  intentional; the one-in-flight behavior is the actual constraint. This now
  also gates `resources/read` and `get_prompt` for native sidecars, not just
  tool calls.
- **`rune self-update` exits 0 even on failure (intentional, per maintainer).**
  The `Commands::SelfUpdate` arm (now in `crates/rune-kit-cli/src/lib.rs`)
  matches the error, `eprintln!`s it, and falls through to `Ok(())` — so a
  failed self-update exits 0 by design. What's changed since the last version
  of this doc: that `Ok(())` no longer *matters* for the exit code the same
  way it used to, since (per the point above) every other command's `Err`
  now also produces exit 0 — self-update's intentional behavior and every
  other command's apparently-unintentional behavior currently look identical
  from outside the process. Worth re-confirming this exception still means
  anything distinct once/if the bug above is fixed.

## 6. Open Questions (not yet confirmed — do not encode as conventions)

- **WASM sandbox: default network policy** — appears to allow-all when no
  `allowed_hosts` is set. Intentional or a gap? (Unchanged since last version.)
- **WASM sandbox: `host_cmd_exec`** — host-command escape hatch exposed to
  plugins. Intentional capability or over-broad? **Now compounded**: it also
  resolves bare program names against the installed native-sidecar plugins
  directory (§2) before failing, meaning a WASM plugin can potentially invoke
  another installed plugin's native binary by name, with no allowlist gating
  that specific resolution path. Whether this was a deliberate convenience
  (e.g. letting a WASM plugin delegate to a co-installed native helper) or an
  unreviewed side effect of adding PATH-fallback logic is unconfirmed.
- **WASM sandbox: `printer_ip` special-case.** Purpose / intent unclear.
  (Unchanged since last version.)
- **CLI exit-code regression (§5)** — is the blanket exit-0 behavior
  intentional (extending self-update's philosophy CLI-wide) or an
  unreviewed side effect of the lib/bin split? Resolve before treating
  either as documented behavior.
- **Graph hygiene:** `arch:two-crate-dual-execution` and `arch:two-crate-structure`
  overlap; candidate to merge into one node. (Unchanged since last version.)

---
*Generated from confirmed graph facts, last updated 2026-09-15. Update via diff-patch as more
facts are confirmed; do not regenerate sections that are already correct.*