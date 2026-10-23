# Public MCP stdio contract

Minimum compatible Kit release: **0.5.0**. Target: [MCP 2026-07-28](https://modelcontextprotocol.io/specification/2026-07-28).

The public gateway uses newline-delimited single JSON-RPC messages. No batches or initialize handshake. Each request, including server/discover, supplies params._meta with `io.modelcontextprotocol/protocolVersion` set to `2026-07-28` and `io.modelcontextprotocol/clientCapabilities` as an object. Optional clientInfo requires name and version. No capabilities are remembered between requests.

server/discover reports supportedVersions, tools/resources/prompts capabilities, private cacheScope and ttlMs 0. Successful results include resultType complete and serverInfo metadata. Cacheable discovery, list, resource and prompt results include ttlMs 0 and private cacheScope. Input schemas default to JSON Schema 2020-12; validation never retrieves external references. Invalid tool arguments return isError true without execution; unsupported schemas fail discovery. Missing/invalid metadata returns -32602; unsupported versions return -32022 with supported/requested data. initialize is unsupported (-32601 when metadata is valid).

Tool names, inputSchema (including x-rune-rules), canonical annotations and legacy aliases are preserved. Read-only and explicit allowlists filter discovery and refuse forbidden calls with -32600. Failed tool discovery returns -32000, never a successful empty list. An empty read-only selection returns tools [] and warns on stderr. Rules resources remain available in read-only mode.

No timeout is imposed by default for native or WASM calls. --timeout sets seconds; expiry returns -32000 with message `Tool execution timed out after N seconds`. Native timeout/cancellation terminates the supervised process; process descendants and blocking host callbacks may take longer and are not guaranteed to stop immediately. Client cancellation sends notifications/cancelled with requestId and receives no further response for that request. IDs distinguish numbers from strings. Legacy $/cancelRequest remains accepted as a notification alias.

The native plugin JSON-RPC operations and WASM exports are a **private plugin ABI**, not this public MCP boundary. Their discovery/call operations retain their existing wire formats. Filesystem guests still require an explicit existing absolute root; missing roots never preopen the working directory.

Shared wire examples live in fixtures/mcp-2026-07-28.json. Linux/macOS execution requires separate verification; see the latest handoff report for exact artifact versions tested. Historical reports are not current verification.
