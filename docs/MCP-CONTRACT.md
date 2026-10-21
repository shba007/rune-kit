# MCP gateway contract

The gateway advertises protocol version `2025-03-26`. Tools are namespaced as
`<namespace>__<tool>`, resources as `rune://<namespace>/<uri>`. Targeted contract
tests are not full MCP conformance certification.

## Tool facts and read-only mode

Annotation keys are `readOnlyHint`, `destructiveHint`, `idempotentHint` and
`openWorldHint`, with optional `title`. Current compatibility output also retains
legacy spellings. Plugin annotations override manifest defaults. `inputSchema`
is opaque JSON, including `x-rune-rules`; a separate top-level `rules` field is
not forwarded. Manifest rules are available through `rune://<namespace>/rules`.

Read-only mode exposes only explicitly declared read-only tools; a forced write
is refused. An empty tools list can also mean an allowlist or discovery failure.
Native fixture tests cover read-only listing, rules-resource access and refusal.
Real hybrid/WASM companion behavior requires separate acceptance checks.

## Timeouts and cancellation

The CLI timeout defaults to no explicit override. Direct native calls fall back
to 30 seconds. WASM has no default per-tool deadline; an explicit `--timeout`
uses Extism cancellation. Hybrid `host_sidecar_exec` independently limits calls
to 60 seconds. Native timeout/cancellation kills and waits for the direct plugin
process and clears pipes; descendant process-tree termination is not guaranteed.
Synchronous host callbacks can delay WASM cancellation or timeout.

| Case | JSON-RPC code | Message |
| --- | --- | --- |
| Cancellation | -32800 | `Request cancelled by client` |
| Timeout | -32000 | `Tool execution timed out after X seconds` |
| Read-only refusal | -32600 | `Permission denied: gateway is running in read-only mode and tool '<name>' is not declared read-only` |

Errors retain the original request ID and have no `data` field. Other runtime
failures also use -32000; clients must not identify timeouts from the code alone.
Cancellation notifications receive no reply, while an interrupted call may
receive its own error response. Completion can race cancellation: discard late
responses by outstanding request ID and avoid reusing IDs.

## Development checks

```text
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --check
cargo build --workspace --locked
```

Set `CARGO_HUSKY_DONT_INSTALL_HOOKS=true` where hook installation is unavailable.
Protocol tests include native fixtures, WASM deadlines and batch notification
handling. A prepared source version is not evidence of a published release;
release metadata and downloadable assets remain owned by release automation.
