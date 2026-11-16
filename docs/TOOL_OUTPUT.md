# Plugin tool output

A tool may declare `outputSchema` (alias `output_schema`) beside `inputSchema`.
It must be a valid JSON Schema with `type: "object"`. Kit reports invalid
output schemas as discovery errors and publishes valid ones as `outputSchema`.
Absent schemas are omitted; Kit does not invent them.

```json
{
  "name": "count",
  "description": "Count items",
  "inputSchema": {"type": "object"},
  "outputSchema": {
    "type": "object",
    "properties": {"count": {"type": "integer"}},
    "required": ["count"]
  }
}
```

WASM returns the following JSON from the existing `mcp_call_tool` export.
Native plugins return it inside the existing private JSON-RPC `result` field.
No private ABI or protocol revision changes are needed.

```json
{
  "content": [{"type": "text", "text": "Found 3 items"}],
  "structuredContent": {"count": 3},
  "isError": false
}
```

Supply the existing content alongside the structured result. Kit preserves that
content array. Native envelopes with structured content are preserved rather than
unwrapping their first text block; legacy successful results keep their existing
handling. Structured content without an output schema is also forwarded.

A successful result with an output schema must include structured content that
matches it. Missing or invalid structured content becomes a tool error naming
the namespaced tool. Successful structured content must be an object.
`isError: true` results bypass output validation, with or without structured content.

Results containing `structuredContent` have an 8 MiB ceiling covering the entire
plugin result, including content. WASM counts its returned UTF-8 JSON bytes;
native counts the serialized JSON result. This also applies to error results
containing structured content. Legacy results gain no size ceiling in this cycle.
Remote MCP passthrough behavior is unchanged.

Run from the Kit repository root:

```powershell
cargo test --locked --test tool_output_tests
cargo test --locked --test remote_http
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```
