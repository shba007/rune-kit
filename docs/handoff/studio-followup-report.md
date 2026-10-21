# rune-kit Follow-Up Handoff Report — Studio KF-1 through KF-7

**Date:** 2026-10-21  
**Branch:** `develop`  
**Commit:** `a907af7` — `feat(protocol): upgrade MCP annotations to spec 2025-03-26 Hint suffix names (KF-1, KF-3, KF-7)`  
**Test counts:** 23 passed (before) → **28 passed, 0 failed** (after). Clippy: 0 warnings. `cargo fmt`: clean.

---

## Items Built vs. Answered

| Item | Decision | Status |
|------|----------|--------|
| KF-1 | **Accept / Built** | Done — annotation wire names fixed, `protocolVersion` bumped, legacy aliases emitted |
| KF-2 | **Accept / Answered** | See below |
| KF-3 | **Accept / Built + Answered** | Done — `rune://<ns>/rules` works in read-only, empty-list warning added |
| KF-4 | **Accept / Answered** | Documented; WASM timeout limitation noted |
| KF-5 | **Accept / Answered** | Cancel semantics documented |
| KF-6 | **Accept / Answered** | Conformance test suite in CI accepted |
| KF-7 | **Accept / Built** | Done — both spellings accepted on input, `x-rune-rules` preserved, top-level `rules` drop documented |

---

## KF-1 — MCP Annotation Wire Names (Must / Built)

### State before this session
`ToolAnnotations` used `#[serde(rename_all = "camelCase")]` which produced `readOnly`, `destructive`, `idempotent`, `openWorld` — all **wrong** per MCP spec `2025-03-26`.

The correct canonical names per MCP spec `2025-03-26` are:
`readOnlyHint`, `destructiveHint`, `idempotentHint`, `openWorldHint`, `title` (optional).

`protocolVersion` in `initialize` was `"2024-11-05"` — also wrong.

### What was built
1. **`crates/core/src/manifest.rs` — `ToolAnnotations` struct rewritten:**
   - Rust field names changed to `read_only_hint`, `destructive_hint`, `idempotent_hint`, `open_world_hint`.
   - Each field uses `#[serde(rename = "readOnlyHint", alias = "readOnly", ...)]` — serialises with canonical `*Hint` name; deserialises from **both** old and new spellings.
   - `title: Option<String>` added (new in spec `2025-03-26`).
   - `From<ToolRuleSpec> for ToolAnnotations` updated to map into `*_hint` fields.

2. **`crates/core/src/runtime.rs` — merge logic updated:**
   - Two occurrences (WASM path and native-sidecar path) updated from `ann.read_only` → `ann.read_only_hint`, etc.

3. **`crates/core/src/protocol.rs` — protocol layer:**
   - `protocolVersion` changed to `"2025-03-26"`.
   - `tools/list` now serialises `ToolAnnotations` with canonical names **and** injects the legacy names (`readOnly`, `destructive`, `idempotent`, `openWorld`) alongside for one-release transitional compatibility.
   - **Drop legacy injection in the next release** (0.2.3 or successor).
   - `tools/call` read-only check updated to use `a.read_only_hint`.

### Changelog note (for release notes)
> **Breaking in next release (0.2.3+):** `tools/list` will stop emitting the legacy annotation names (`readOnly`, `destructive`, `idempotent`, `openWorld`). Only the canonical MCP spec `2025-03-26` names (`readOnlyHint`, `destructiveHint`, `idempotentHint`, `openWorldHint`) will be present. Clients should migrate before then.

### New tests
- `test_annotation_wire_names_canonical` — verifies `ToolAnnotations` serialises with `*Hint` names only (legacy not in `ToolAnnotations` JSON itself).
- `test_annotation_both_spellings_deserialise` — verifies both `readOnlyHint` (new) and `readOnly` (old) are accepted during deserialisation.

---

## KF-2 — Release Question (Should / Answer)

### Q: What is the target release for KF-1 and KF-7? What is the Windows asset name? What does `rune --version` report?

**Target release:** `0.2.2` (current workspace version in `Cargo.toml`). No version bump was made here; the annotation fix goes into the `0.2.2` development cycle on `develop` before it is promoted to `main`.

**Windows asset name:** Determined from `.github/workflows/deploy.yml`:
```
rune-<VERSION>-x86_64-pc-windows-msvc.zip
```
e.g. `rune-v0.2.2-x86_64-pc-windows-msvc.zip`. The archive contains `rune.exe` at its root.

**`rune --version` output:** `rune 0.2.2` (reported by `clap` from the package version in `Cargo.toml`).

---

## KF-3 — Read-Only Mode and `rune://<ns>/rules` (Should / Built + Answered)

### `rune://<ns>/rules` in read-only mode
`resources/list` always includes `rune://<ns>/rules` for every registered namespace — the rules resource is a **read resource**, not a tool. It is never filtered by `--read-only`. Confirmed in `protocol.rs`: the rules `push` is in the `resources/list` handler, not behind any `config.read_only` check. `resources/read` for `rune://<ns>/rules` returns the full manifest JSON regardless of `--read-only` mode.

### Hybrid plugin annotations
A plugin that declares some tools `read_only = true` in `plugin.toml` and others not will expose **only the read-only tools** in `tools/list` under `--read-only` mode. The annotations merge logic (manifest override → plugin-declared → no annotation) works correctly in both the WASM and native-sidecar paths.

### Empty read-only tool list
When `--read-only` is active but no tool from any registered plugin declares `readOnlyHint = true`, `tools/list` returns `[]` (an empty array, not an error). A **stderr warning** is now emitted to alert the operator:

```
[Rune Warning] --read-only mode is active but no tools are declared readOnlyHint=true.
The tools list exposed to clients is empty. Declare read_only = true in plugin.toml
[tools.<name>] to expose tools in read-only mode.
```

This is written to stderr; stdout JSON-RPC remains clean.

### New tests
- `test_router_resources_list_empty_no_instances` — verifies `resources/list` returns empty array with no plugins (not an error).
- `test_router_tools_list_read_only_empty` — verifies empty `[]` is returned in read-only mode with no registered instances.

---

## KF-4 — Timeout Defaults, Kill Behaviour, Error Shapes (Should / Answer)

### Timeout defaults (verified in code, not just claimed)

**WASM (`WasmPluginInstance::call_tool_with_cancel`):**
- The `_timeout` parameter (prefixed `_` intentionally) is **not implemented** for WASM. Passing `--timeout 30` has no effect on WASM plugins.
- **NOT CERTAIN on workaround:** Extism/Wasmtime can be cancelled via `CancelHandle::cancel()`, which is already wired for cancel-on-request. A timeout-driven cancel would require a timer thread calling `cancel_handle.cancel()` after the deadline — this is not yet implemented.
- WASM calls may block indefinitely if the guest hangs.

**Native sidecar (`NativeSidecar::send_request_with_cancel`):**
- Poll loop runs at 100 ms intervals.
- `timeout_duration = timeout.unwrap_or(Duration::from_secs(30))` — the effective default is **30 seconds** when no `--timeout` is specified.
- If the timeout expires, returns `RuntimeError::Timeout(secs)`.

### Kill behaviour
- **Native:** `send_request_with_cancel` returns `Err(RuntimeError::Timeout(...))` after the deadline; the process is **not killed** immediately — it is killed lazily on the next call or on `Drop`. Standalone native processes: same.
- **WASM:** `CancelHandle::cancel()` signals Wasmtime to abort; this is cooperative and not guaranteed to be immediate.

### Exact error shapes

| Code | Scenario | Message example |
|------|----------|-----------------|
| `-32700` | Parse error (malformed JSON) | `"Parse error"` |
| `-32601` | Unknown method | `"Method 'foo' not found"` |
| `-32602` | Missing namespace separator in tool name | `"Invalid tool namespacing (expected namespace__tool)"` |
| `-32600` | Allowlist rejection | `"Permission denied: tool 'ns__t' is not in the allowlist"` |
| `-32600` | Read-only rejection | `"Permission denied: gateway is running in read-only mode and tool 'ns__t' is not declared read-only"` |
| `-32800` | Pre-cancelled (ID already in cancelled set) | `"Request cancelled by client"` |
| `-32800` | In-flight cancel (flag set during execution) | `"Request cancelled by client"` |
| `-32000` | Timeout | `"Tool execution timed out after 30 seconds"` |
| `-32000` | Other runtime error | `e.to_string()` from `RuntimeError` |

### New test
- `test_error_shapes` — asserts the exact codes for `-32800` (cancelled), `-32600` (allowlist), `-32601` (unknown method), and `-32700` (parse error) from live router calls.

---

## KF-5 — Cancel Response Semantics (Question / Answer)

**Q: After `notifications/cancelled`, does kit return a response for the cancelled call?**

**Yes.** kit always emits a `-32800` JSON-RPC error response for a cancelled call:
- **Pre-cancelled** (ID registered in `cancelled_ids` before the call arrives): the router detects the ID at the start of `tools/call` handling and immediately returns `{ "error": { "code": -32800, "message": "Request cancelled by client" } }`.
- **In-flight cancel** (cancel notification arrives while call is executing): the cancel flag is set, `call_tool_with_cancel` returns `Err(RuntimeError::Cancelled)`, and the router maps this to `{ "error": { "code": -32800 } }`.

**Impact on Studio:** The MCP spec says a client that sends `notifications/cancelled` SHOULD NOT wait for a response. Studio is safe: a late `-32800` arriving after Studio has already moved on is harmless. Studio must not treat a late `-32800` as an unexpected new error — it is the (optional) echo of the cancel Studio itself initiated.

---

## KF-6 — Studio Conformance Test in CI (Nice / Answer)

**Accept.** rune-kit's CI pipeline (`integrate.yml`) runs `cargo test --workspace`, `cargo clippy -- -D warnings`, and `cargo fmt -- --check` on every push to `develop`. Studio is welcome to supply a conformance test suite (JSON-RPC request/response sequences against `rune run --all`) as a separate script. The repo will integrate it as a step in `integrate.yml` under a `conformance` job. Preferred form: an executable script or binary that exits 0 on success, so CI can invoke it as `cargo build --release && conformance-suite target/release/rune`.

---

## KF-7 — Annotations from Plugins in Both Spellings (Must / Built)

### State before this session
`ToolAnnotations` only accepted `readOnly` (old spelling) because `rename_all = "camelCase"` was the only serde directive. The new `readOnlyHint` spelling from a spec-compliant plugin would have been silently dropped (field not matched, default `None` used).

### What was built
The `serde(alias = ...)` on each field (see KF-1 above) now accepts **both spellings** when deserialising from plugin `tools/list` responses:

```json
// Both of these are now accepted and produce the same ToolAnnotations:
{"readOnlyHint": true}   // spec 2025-03-26 plugin
{"readOnly": true}        // legacy plugin
```

### `inputSchema` / `x-rune-rules` preservation
`ToolDefinition.input_schema` is typed as `serde_json::Value` — it is entirely **opaque** to rune-kit. Any field inside it, including `x-rune-rules`, is round-tripped verbatim. This is confirmed by inspection: no code transforms or filters `input_schema` content between deserialisation from the plugin and serialisation into `tools/list`. Studio will receive `x-rune-rules` exactly as the plugin declared it.

### Top-level `rules` field (plugin tool object)
A top-level `rules` field on a tool object in a plugin's `tools/list` response is **silently dropped** because `ToolDefinition` has no `rules` field and `serde` defaults to ignoring unknown fields. Studio has confirmed this is acceptable as long as `x-rune-rules` inside `inputSchema` survives. That is the case. This behaviour is now documented here.

### `rune://<ns>/rules` and plugin-declared tool facts
`resources/read` for `rune://<ns>/rules` returns:
```json
{
  "namespace": "<ns>",
  "manifest": { <full PluginManifest as JSON> }
}
```
The `manifest.tools` map contains the `[tools.<name>]` entries from `plugin.toml` with all declared `ToolRuleSpec` fields (`read_only`, `destructive`, `idempotent`, `open_world`, `description`). This is the authoritative per-tool fact sheet for Studio's safety gate. Note that these are the rune-kit-side overrides; what the plugin itself declares in its `tools/list` annotations is merged in at call time but is not re-serialised into the rules resource (the resource reflects `plugin.toml` only, which is intentional — it is the operator-auditable ground truth).

---

## Corrections to the Studio Handoff Document

| Claim in handoff | Actual state |
|------------------|--------------|
| "rune-kit uses `readOnly`, `destructive`, `idempotent`, `openWorld` wire names" | **Was true; now fixed.** Canonical names are `readOnlyHint` etc. per this commit. |
| "`protocolVersion` is `2024-11-05`" | **Was true; now fixed** to `2025-03-26`. |
| "WASM timeout is honoured" | **NOT CERTAIN / not implemented.** The `_timeout` param is accepted but ignored for WASM. |
| "Windows asset: `rune-x86_64-pc-windows-msvc.zip`" | **Partially wrong.** Actual name includes the version: `rune-v<VERSION>-x86_64-pc-windows-msvc.zip`. |

---

## Response Table

| Item | Response | Notes |
|------|----------|-------|
| KF-1 | **Accept** — built in `a907af7` | Wire names fixed; both spellings emitted (canonical + legacy) for one release |
| KF-2 | **Accept** — answered | Release: `0.2.2`; Windows asset: `rune-v0.2.2-x86_64-pc-windows-msvc.zip`; `rune --version`: `rune 0.2.2` |
| KF-3 | **Accept** — built + answered | `rune://<ns>/rules` always in resources; empty read-only list → `[]` + stderr warning |
| KF-4 | **Accept** — answered | Native default: 30 s. WASM timeout: NOT IMPLEMENTED (documented). Error shapes: table above |
| KF-5 | **Accept** — answered | kit sends `-32800` for cancelled calls; Studio may safely ignore late responses |
| KF-6 | **Accept** — answered | Conformance test suite welcome; will be added to `integrate.yml` as a `conformance` job |
| KF-7 | **Accept** — built in `a907af7` | Both spellings accepted; `x-rune-rules` in `inputSchema` preserved; top-level `rules` drop documented |
