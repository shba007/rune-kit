# MCP HTTP contract

First planned HTTP-capable Kit release: **0.6.0**. Published 0.5.0 is NOT an HTTP
client compatibility guarantee. This working tree still reports its pre-release
workspace version until owner-pushed release preparation updates it. Target:
https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/streamable-http

## Startup and endpoint

`rune-kit serve` loads all installed plugins at startup, or select a positional
installed name/local artifact. Default **http://127.0.0.1:47601/mcp**. Optional
--port/--bind allow another loopback address only. --port 0 is useful for tests;
the actual endpoint is printed on stderr. Missing/broken payloads fail startup
rather than silently removing tools. An empty installation serves empty discovery.
Install/update/list/self-update and shared ~/.rune/tools storage remain available.
Restart the server after changing installed plugins or server configuration.

Interactive serve opens the attached TUI. --headless suppresses it; non-interactive
serve is headless automatically. q/Esc/Ctrl+C in the dashboard, Ctrl+C headlessly,
stop accepting work and cancel queued/active requests before draining connections.
Unix background processes also respond to SIGTERM (Linux/macOS execution NOT CERTAIN).
Internal native subprocess descendants/blocking host callbacks cannot be guaranteed
to stop immediately. No HTTP shutdown endpoint or remote administration is exposed.

## Requests

One JSON-RPC request per POST /mcp, no batches or initialize handshake. Required:

- Content-Type: application/json
- Accept: application/json, text/event-stream (both, with positive q values)
- MCP-Protocol-Version: 2026-07-28
- Mcp-Method: exact body method
- Mcp-Name: exact params.name for tools/call/prompts/get or params.uri for resources/read
- params._meta contains io.modelcontextprotocol/protocolVersion and an object
  io.modelcontextprotocol/clientCapabilities. Optional clientInfo needs name/version.

Mcp-Name and annotated Mcp-Param-* values support the specification's UTF-8 Base64
sentinel encoding. Required standard/mirrored headers must match the body or fail
HTTP 400 / -32020. Unsupported version fails HTTP 400 / -32022 with supported and
requested data. Unknown methods fail HTTP 404 / -32601. Missing capabilities,
invalid envelopes, malformed JSON and oversized bodies fail explicitly. Bodies
are limited to 256 KiB with a 10-second read deadline. GET/DELETE return 405;
there are no protocol sessions, resumable streams or independent GET event stream.
Mcp-Session-Id and Last-Event-ID are ignored, never minted/echoed.

Valid requests receive an SSE response scoped to that request: `event: message`
and a single `data: <JSON-RPC response>` final frame, then EOF. Clients must also
accept application/json, used for early errors. Some dispatch errors (e.g. invalid
arguments) arrive as a normal SSE JSON-RPC error or tool isError result; inspect
both HTTP status and JSON-RPC/tool results. No success is inferred from an empty
response. Only complete results are implemented; no tasks, subscriptions, MRTR
input requests or independent server notifications are advertised.

server/discover returns supportedVersions, tools/resources/prompts capabilities,
resultType complete and serverInfo in result metadata. Discovery/list/resource/
prompt results have private cacheScope and ttlMs 0. Native/WASM schemas,
x-rune-rules, canonical annotations and supported legacy aliases are preserved.
Invalid tool discovery (including malformed x-mcp-header annotations) fails with
-32000 rather than returning a successful empty inventory.

## Permissions and configuration

--read-only, --allow-tool and `-p key=value` are fixed server configuration.
Filesystem guests require an existing explicit absolute allowed_dir; requests
cannot replace this root. Installed default_params are overridden by supported host parameters, then explicit
-p values. Each plugin receives a separate fixed copy at startup. The dashboard
shows resolved allowed_dir values. No request changes this configuration. All
shared plugin execution is serialized. Queue capacity is 32; saturation returns 503.
The queue/discovery admission deadline is 35 seconds. Actual calls use --timeout
(default 30 seconds); timeout returns -32000 and native tool execution is killed.
Discovery and WASM operations also have cancellation/deadline guards. Native
resource/prompt operations share the same operation cancellation/deadline context;
private native ABI callers outside HTTP retain their 30-second fallback.

Clients may optionally send params._meta["io.rune-kit/allowedTools"] as an array
of exact tool names. It only narrows tools/list and tools/call access; absent means
server-permitted access, [] means no tools. It never overrides server restrictions.
Forbidden calls return -32600. Arguments are still checked against discovered
schemas; unsupported schema/discovery failures remain errors. No HTTP request
body supplies allowed_dir, process environment, or a new permissions ceiling.

Each request has a separate internal operation identity and cancellation flag;
JSON-RPC IDs may be reused by different clients without collisions. Cancel by
closing that request's SSE response. No further result is sent for a disconnected
request, and uncertain operations are never replayed automatically. HTTP core
notifications/cancelled and $/cancelRequest are rejected: those are not the
2026-07-28 HTTP cancellation mechanism. Queued cancelled requests never execute.

The server rejects every Origin-bearing browser request and requires a Host
matching its bound loopback endpoint (or localhost at the same port). It binds
only loopback. Optionally set RUNE_KIT_HTTP_TOKEN to require Bearer authorization;
Studio must send the same token locally. No token is generated or exposed by the
TUI. Without it, local processes are trusted; this is not remote/multi-user hosting.

## Studio migration and fixtures

Studio must replace subprocess spawning with a configured endpoint and optional
local bearer token, and require the HTTP-capable Kit release (planned 0.6.0).
The client owns selected tools and narrows requests; server permissions remain
the ceiling. Handle SSE final frames, HTTP errors, MCP errors and tool isError;
close the response stream on user cancellation and discard uncertain results.
Never retry a tool operation automatically after disconnect.

See fixtures/mcp-http-2026-07-28.json for concrete header/body examples, including
version mismatch, client narrowing, forbidden writes and disconnect semantics.
No native/WASM plugin ABI changes are required. Old public `rune-kit run` fails
with migration instructions; internal native JSON-RPC pipes remain unchanged.
The rune-tools Postman collection must be migrated separately by its owner;
this assignment does not modify sibling repositories.
