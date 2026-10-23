# MCP gateway contract

The CLI executable is `rune-kit` (`rune-kit.exe` on Windows). User data remains
in the existing `rune-kit` data directory; no `rune` alias is installed.

The gateway advertises protocol version `2025-03-26`. Tools are namespaced as
`<namespace>__<tool>`, resources as `rune://<namespace>/<uri>`. Targeted contract
tests are not full MCP conformance certification.

## Tool facts and read-only mode

Annotation keys are `readOnlyHint`, `destructiveHint`, `idempotentHint` and
`openWorldHint`, with optional `title`. Current compatibility output also retains
legacy spellings throughout 0.4.x as well as 0.3.x. Output removal requires a minor-version release
and an explicit migration note; legacy input stays supported. Both spellings may
appear in one object if their boolean values agree. Conflicting pairs reject the
plugin's discovery response, naming both keys. Non-boolean declarations (including
null) are rejected; absent optional facts remain unknown, never implicitly read-only.
The centralized ToolAnnotations decoder is shared by native and WASM discovery.
Plugin annotations override manifest defaults. `inputSchema`
is opaque JSON, including `x-rune-rules`; a separate top-level `rules` field is
not forwarded. Manifest rules are available through `rune://<namespace>/rules`.

Read-only mode exposes only explicitly declared read-only tools; a forced write
is refused. An empty successful tools list means no discovered tools survived the
filters (or plugins genuinely declared none). Any plugin discovery failure returns
JSON-RPC -32000 with `Tool discovery failed: plugin '<namespace>': <reason>` instead
of a successful empty or partial list; stderr also records the failure. This applies
even when other plugins discovered tools successfully. Read-only forced calls keep
the existing fail-closed -32600 behaviour if their facts cannot be discovered.
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

## Studio Cycle 2 invocation

Prefer `rune-kit --version` (0.4.0+). The released fallback is `rune --version`
(0.3.1); discover both commands without assuming the new one is installed.
Use `rune-kit run <installed-name-or-artifact> --read-only --allow-tool get_current_time --timeout 2`.
Allowlist entries may be local tool names or complete names returned by tools/list.
Never construct a namespace from assumptions: use the returned tool name.
Initialize with protocolVersion 2025-03-26, capabilities {}, and clientInfo;
then send notifications/initialized, then tools/list and tools/call with discovered schema.
Discovery failures have -32000 and a message starting `Tool discovery failed:`;
timeouts have -32000 and `Tool execution timed out after X seconds`; other runtime
failures also share -32000. Do not classify by code alone. Cancellation carries
params.requestId, emits no response to the notification, and an interrupted call
can still produce -32800 with its original ID. Discard late replies by request ID.

For the filesystem pilot configure the host root, never model arguments:
`rune-kit run rune-filesystem-0.4.0 --read-only -p allowed_dir=D:/Pilot`.
The root must be an existing absolute host directory. Kit maps precisely that
host string to the same guest path and forwards allowed_dir unchanged as plugin
configuration. Use forward slashes on Windows and relative call paths such as
hello.txt or . . Missing configuration creates no WASI preopen; invalid roots
are rejected. No working-directory fallback remains in 0.4.0.
The released filesystem 0.4.0 artifact declares readOnlyHint=false for read_text_file
and list_directory: both are deliberately unavailable in read-only mode until
Tools fixes its annotations. Normal-mode diagnostic calls succeeded through an
installed WASM artifact in isolated APPDATA; this is not pilot approval.
