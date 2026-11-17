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

## Host embedding: plugin filesystem folders

Only a Rust host holding `server::EmbeddedServer` may change an embedded
plugin's folder while Kit is running:

```rust,ignore
pub fn set_plugin_param(&self, plugin: &str, key: &str, value: &str) -> Result<(), String>;
pub fn plugin_params(&self, plugin: &str) -> std::collections::BTreeMap<String, String>;
```

The setter accepts `allowed_dir` or a manifest configuration field whose
`config_key` is `allowed_dir`. Other keys, unknown plugins, secret root fields,
and workspace-only plugins are refused. The path must be an existing absolute
directory. The embedding-only validator canonicalises it, storing and displaying
normal Windows drive/UNC paths without the `\\?\` prefix. It does not impose a
new drive-root restriction. Setup currently excludes these fields and has no
folder validator; its UI and validation are unchanged.

The value is saved per plugin in `dashboard.json`, using setup's `dashboard.lock`
and atomic writer. An active save, externally changed dashboard, invalid folder,
load failure, or save failure returns an error without replacing the old settings
or instance. Retry busy errors after the current operation/save finishes; an
external dashboard change requires restarting to load it. Do not call this
synchronous filesystem/plugin-loading operation on an async executor worker;
use the host's blocking worker. Only the chosen plugin is reloaded between
requests; the HTTP listener and other plugins stay running. Native plugins keep
their existing enforcement limitations; this API does not introduce OS isolation.

`plugin_params` returns declared non-secret configuration plus the effective
`allowed_dir` (and its declared alias), never arbitrary process environment or
secret fields. Unknown plugins return an empty map. Default embedded startup
loads saved per-plugin folders. A host constructing its own router for
`start_embedded` remains responsible for its initial plugin instances.

No MCP or HTTP route exposes the setter. Tool arguments cannot replace the root,
and `/management/config` and the TUI continue refusing filesystem root edits.
Ordinary `serve` startup and setup retain their existing configuration handling;
this folder override is an embedding facility.

Before this API, `start_default_embedded` passed no explicit parameters, but
its installed-environment loader merged installed `default_params`, saved plugin
secrets, and supported host environment parameters (including `ALLOWED_DIR` and
`RUNE_PARAM_ALLOWED_DIR`). With no supplied root it installed no WASI filesystem
preopen; it did not infer a folder from CWD. The new API supplies a per-plugin
root without requiring process-wide environment changes.

## Host embedding: staged plugin file access

An embedded host can run chosen WASM plugins in staged mode without global
`--workspace-only` (which disables remotes):

```rust,ignore
pub fn set_plugin_staged(&self, plugin: &str, staged: bool) -> Result<(), String>;
pub fn is_plugin_staged(&self, plugin: &str) -> bool;
```

Prerequisites: a known WASM plugin with a configured `allowed_dir` (set it first with
`set_plugin_param`). Enabling canonicalises that folder, approves it for the plugin in
the sandbox inventory (`approvals.json` under `paths::sandbox_root()`), and reloads only
that plugin between requests. Errors (unknown plugin, native plugin, no folder, busy
server, plugin already staged by `--workspace-only`) keep the previous instance. The
setting is per server instance and not saved; call it again after a restart. Later
`set_plugin_param` folder changes keep the plugin staged and approve the new folder.
Turning staging off reloads the plugin for direct access; the approval and any pending
changes remain reviewable. Other plugins and remotes are untouched. Like
`set_plugin_param`, this is synchronous: use the host's blocking worker.

A staged plugin gets no WASI preopen of the real folder (its `/workspace` is an empty
private directory) and no `host_cmd_exec`, `host_sidecar_exec` or `host_get_binary_path`
imports. It receives two extra config keys: `rune_staged` = `"true"` and
`rune_staged_root` = the real absolute folder. The guest must route every access
through the `host_file_access` import with absolute paths under that folder. Unstaged
plugins now also get a `host_file_access` that always errors, so one `.wasm` can import
it and still load in both modes; it should check `rune_staged` and use WASI otherwise.

### Broker request (schema_version 1, additive)

`host_file_access` takes JSON `{"schema_version":1,"operation":…,"path":…,"data":[bytes]}`
and returns `{"schema_version":1,"staged":bool,"data"?,"entries"?,"stat"?}`.

| operation | staged work | result |
| :--- | :--- | :--- |
| `read_file` | no | content, staged content when the file has pending changes |
| `write_file` | yes | parent folder must exist (possibly as staged) |
| `delete_file` | yes | staged as a deletion |
| `list_directory` | no | names with staged additions and deletions applied |
| `stat` | no | `{exists, kind: file or directory, size?, modified?}` of the staged view |
| `create_directory` | yes | no-op if present; parent must exist (possibly staged) |
| `remove_directory` | yes | only if empty in the staged view |

`stat`, `create_directory` and `remove_directory` are new. Read-back is guaranteed: a
read, stat or listing after a staged write, delete, create or remove sees the staged
state; the original is not touched. Plugins build the rest from these:

- Make parents: one `create_directory` per level (shallowest first). `mkdir -p` is a
  loop; existing folders are accepted.
- Move or rename a file: `read_file` source, `write_file` destination, `delete_file`
  source. Moving a folder: `create_directory`, recurse, `remove_directory` of the
  emptied source. Each step is a separate reviewable item. The 16 MiB per-file broker
  limit applies. There is no single atomic "move" item.
- Metadata: `stat`; listing gives names only.

### Review, apply and undo

Changes appear in `GET /management/changes` as before. Each review now has
`kind: "file" | "directory"`; recoveries carry `kind` too (directory recoveries have no
backup file). `deleted: true` is a deletion or folder removal. Only the host can act on
them, through `POST /management/changes` with the exact `approval` token of a current
item; guest requests with any other operation (including `apply`) are rejected. A token
goes stale if the staged content, the baseline or the kind changes, and apply is refused
if the original changed since it was first observed. Folders must be applied before
their contents (otherwise "Apply the parent folder first", with nothing left to
resolve), and removed folders must be empty when applied. `restore` undoes an applied
item: files from their backup, created folders by removing them if empty, removed
folders by recreating them. Undo children before parents.

Not covered: the broker accepts only regular files and folders, refuses symlinks and
junctions, and has no permission or timestamp changes. Remotes are not plugins in the
embedded configuration, so staging never touches them.
