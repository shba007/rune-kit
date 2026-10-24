# Local MCP HTTP server and release handoff

Verified on Windows, 2026-10-24 (Asia/Calcutta). Base develop revision:
90423f0ca1f260144ef4a0a3aaf70d9aba413522. Work is local, unstaged and uncommitted;
no branch, commit, push, merge, tag, publication or workflow dispatch was performed.
This handoff modifies rune-kit only. Studio client and rune-tools Postman changes
require their own assignments.

## Changes and user-visible behavior

- Canonical `rune-kit serve` exposes **http://127.0.0.1:47601/mcp**. Interactive
  terminals attach the existing Rust TUI; `--headless` and non-interactive serving
  work without a terminal. No public stdio serving remains: legacy `run` exits
  with HTTP migration guidance. Install/list/update/self-update and the shared
  binary/data roots remain available; private native/WASM ABI is unchanged.
- Implemented MCP 2026-07-28 Streamable HTTP: POST requests, per-request metadata,
  standard/header-body validation, Base64 header values, x-mcp-header validation,
  SSE final responses and JSON early errors, discovery, explicit errors, request
  correlation and disconnect cancellation. GET/DELETE return 405. No sessions,
  resumable streams, independent notifications, tasks or unsupported capabilities.
- Host validation, rejection of browser Origins, loopback-only bind and optional
  RUNE_KIT_HTTP_TOKEN Bearer authentication. Body limit 256 KiB/read deadline 10 s;
  serialized dispatch queue capacity 32, admission deadline 35 s, configurable
  tool/operation timeout default 30 s. Native cancellation kills the supervised
  process. Only the active WASM instance receives cancellation, not idle guests.
- Server read-only/allowlist/root configuration is a ceiling. Per-request
  io.rune-kit/allowedTools can only narrow access. Installed defaults, supported
  host parameters and explicit -p overrides become separate immutable per-plugin
  configurations; arguments cannot replace the filesystem root. Corrupt installed
  metadata and missing payloads are actionable startup errors, not empty success.
- Attached TUI shows observed endpoint/bind state, uptime, installed versions and
  runtime type where known, discovery status, active/recent operations, durations,
  failure categories/codes and effective restrictions. History is bounded to 64 entries;
  it contains no tool arguments, results or tokens. It monitors only this server.
  Arrows/j/k select, r refreshes, q/Esc/Ctrl+C stop the server. Standalone `tui`,
  --once and --json remain inventory views, not remote server monitors.
- Release preparation now runs only after successful develop-push checks, handles
  semantic maintenance/custom commits as well as features/fixes, validates generated
  files in that run, uses GITHUB_TOKEN for PR operations and GH_TOKEN for branch
  pushes, suppresses stale preparation and reuses release/update-version. Main
  promotion requires a merge commit titled ci(release): publish vX.Y.Z. Publication
  extracts the exact prepared CHANGELOG section. No AI service participates.

## Changed files

- crates/cli/src/server.rs (new HTTP server); lib.rs (serve/legacy migration and
  startup configuration); tui.rs (attached dashboard); crates/cli/Cargo.toml.
- crates/core/src/protocol.rs (transport-independent cancellable dispatch, active
  WASM guard, removal of stdio loops); runtime.rs (native operation context);
  crates/core/tests/protocol_tests.rs (cross-plugin WASM cancellation regression).
- crates/cli/tests/http_and_tui.rs replaces stdio_and_tui.rs; output_tests.rs
  checks serve help. xtask/src/wire.rs is now an actual HTTP test client, retaining
  only private native pipes for ABI checks; xtask/Cargo.toml, main.rs, release.rs.
- Cargo.lock, CHANGELOG.md, README.md, AGENTS.md, cog.toml; docs/MCP-CONTRACT.md,
  docs/TUI.md, docs/RELEASING.md and this report; fixtures/mcp-http-2026-07-28.json.
- .github/workflows/checks.yml, integrate.yml, release.yml; removed the obsolete
  .github/release-commits.tera. deploy.yml was validated without changing it.

## Actual validation

`cargo test --locked --workspace`: **77 passed, 0 failed**:
6 CLI unit; 12 HTTP/TUI process; 9 installer; 7 CLI output; 5 core unit;
6 annotation; 22 protocol; 2 security; 8 Rust release utility tests.

`cargo clippy --workspace --all-targets --locked -- -D warnings`, cargo fmt
--all --check and git diff --check passed. actionlint 1.7.12 passed all four
workflows (optional shellcheck/pyflakes disabled). No publishing action ran.

The HTTP process tests exercised real loopback requests: discovery/list/call,
read-only and explicit server allowlist refusal, client narrowing, header/Origin/
Host/content-negotiation checks, mirrored parameters, token authentication,
timeouts, stream disconnects, independent clients reusing IDs, occupied ports,
legacy command rejection, shared Studio fixture responses, non-interactive
serving with closed stdin and corrupt metadata. The independent WASM regression
cancelled one spinning guest and successfully called another using the same ID.

Actual ConPTY entry/exit tests covered standalone empty/failed discovery and the
attached HTTP server. A real HTTP ping appeared in the attached dashboard, then
q shut it down and restored the alternate screen. Raw evidence is in ignored
`target/tui-http-terminal.txt`, with standalone evidence alongside it. No desktop
screenshot was captured. A raw-ANSI substring assumption initially failed in the
PTY test; the corrected check looks for the server panel and live ping history.

`cargo xtask conformance` passed using real native Git, WASM Time and WASM
Filesystem artifacts through HTTP. Verified commit-message validation, preserved
Git schemas, UTC time, in-root read/list, outside-root refusal even with a request
root override, malformed input handling and write refusal. Result evidence:
`target/http-conformance.json`. These were isolated test registries/temporary
roots; no owner installation or Infer files were changed.

| Tested artifact | Filename | SHA-256 |
| --- | --- | --- |
| Kit development binary (reports 0.5.0; HTTP changes uncommitted) | `rune-kit.exe` | `a32d3b019a14256d632e2833001dad4a3c2409184b7fce02a47b48fa628897ff` |
| Git native 0.3.0 | `rune-git-native.exe` | `6b9af56d1f8daf7c586909bda831dcbd3a7f32fafbe370497def153fb2716f9b` |
| Time WASM 0.3.0 | `rune-time-0.3.0.wasm` | `a80fa8c15bc63df2078ef4fa371395efed2d1bf312889e84530f3a67919c68f7` |
| Filesystem WASM 0.4.1 | `rune-filesystem-0.4.1.wasm` | `b513ddd8fc248d436d4b4aa3904a49885239ab75ca8d59550c6fc5b756c2d067` |

The development binary's reported 0.5.0 is not a claim that the published 0.5.0
release has HTTP support. The first planned compatible HTTP release is **0.6.0**;
CI updates manifests/lockfile after the owner pushes the reviewed breaking change.

## Migration, limits and unverified behavior

Existing ~/rune executables, ~/rune/tools storage/plugins, explicit path overrides,
retained-original migration and Infer coexistence are preserved. Installer tests
(including PowerShell 5.1 and Git Bash on Windows), interrupted/conflicting
storage migration tests and self-update archive recognition passed. No fresh
0.6.0 released asset can be installed or self-updated until the owner publishes it.

Studio must use an HTTP endpoint rather than spawn run/stdio, require the HTTP
release, send required headers and metadata, parse SSE or JSON responses, and
cancel by closing the response stream. Shared fixtures and the complete error/
configuration contract are in docs/MCP-CONTRACT.md. The old stdio Postman
collection in rune-tools needs a separately scoped transport update.

Only local trusted-process hosting is supported. Native descendants and blocking
host callbacks cannot be guaranteed to terminate immediately. Plugin execution
is intentionally serialized. Changes to installed plugins/config require restart.
The dashboard reports accepted dispatch calls; it is not a traffic/security log
or a monitor for Studio/other Kit processes. No Screen Insight orchestration or
future workflow engine was implemented in this repository.

Linux/macOS runtime, IPv6 binds, headless Ctrl+C/SIGTERM signaling, forced
render/input failures and panic cleanup, live Studio HTTP integration,
owner desktop/model workflows, remote token grants/rulesets and actual GitHub
release/artifact event chaining are **NOT CERTAIN** (not executed here).

## Release previews and owner actions

Two release-plan previews and two synthetic release-example previews were
byte-identical. The real committed-history plan correctly says **No releasable
changes**, version 0.5.0: these HTTP edits are still uncommitted. The synthetic
breaking example calculates **0.6.0** without Git mutation. Exact proposed 0.6.0
notes extracted from CHANGELOG.md (also target/http-release-notes.md):

```markdown
- Serve public MCP 2026-07-28 over a persistent loopback HTTP endpoint with an attached terminal dashboard.
- Replace public `rune-kit run` stdio serving with `rune-kit serve`; clients must connect to `http://127.0.0.1:47601/mcp` and send matching MCP metadata headers.
- Use `--headless` for CI/background operation and close a request's SSE stream to cancel it. Native/WASM plugin ABI and shared installation/storage paths remain unchanged.
- Preserve server permission ceilings, read-only filtering, explicit filesystem roots and independent cancellation while serializing shared plugin execution.
- Prepare releases after validated develop pushes, reuse one owner-reviewed preparation PR, and publish the exact prepared notes after a merge-commit promotion to main.
```

Suggested owner commit subject:
`feat!: serve local MCP over HTTP with an attached dashboard`

1. Owner reviews and commits/pushes these changes to develop. Successful Windows/
   Ubuntu checks lead to deterministic preparation and validation in that run.
2. Review/merge release/update-version → develop (merge or squash). This does NOT
   publish. Verify the originating preparation run and develop merge validation.
3. Promote develop → main with Create a merge commit, preserving ancestry, titled
   **ci(release): publish v0.6.0** (or the actual prepared version if it differs).
4. Main validates prepared history/notes, publishes tag/release using GH_TOKEN;
   release.published triggers artifact builds. Owner verifies all platform assets
   and installation before Studio enables its HTTP integration.

Required settings: allow Actions to create PRs; preparation job Contents/Pull
requests write for GITHUB_TOKEN; GH_TOKEN with repository Contents write for
branch pushes and publication; permitted third-party Actions/bot branch writes;
owner review and merge commits enabled. Do not require an untriggered PR-only
check on the release branch. GH_TOKEN release events must reach deploy.yml;
GITHUB_TOKEN-only publication would suppress that downstream event.

Remaining non-Rust tooling: thin POSIX/PowerShell bootstrap installers are needed
before a Rust binary exists; Actions YAML, GitHub CLI and jq wire events together.
Go-based actionlint is a local validator, not product/release-generation code.
Repository-owned release logic and protocol/migration/installer tests are Rust.
