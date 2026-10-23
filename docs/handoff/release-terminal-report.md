# Terminal inventory and automated release preparation

Implemented locally in Rune Kit on 2026-10-23. No source commit, branch, tag, push, merge, release publication or publishing-workflow dispatch was performed by the agent. The owner commits/pushes development work and reviews/merges release and promotion PRs. The index is cleared before delivery; final changes remain unstaged and uncommitted.

## Implementation

- `rune-kit tui` is explicit and independent of `run`. Ratatui styling follows Rune Infer's cyan accent, muted rounded panels, clear inventory/details and visible key hints. Normal CLI and MCP output paths remain separate.
- Inventory shows resolved executable/storage paths, installed versions and recorded Native/WASM execution, actual tool discovery, annotations, declared capabilities, configured roots, and actionable missing payload/dependency or malformed-metadata errors. Probes stop after discovery; no tools are called, provisioned or installed. No CPU, health, request history or visibility into Studio's separate gateway is invented.
- Navigation: arrows/j/k select, Tab changes focus, arrows/j/k or PgUp/PgDn scroll details, Home resets, r refreshes, q/Esc/Ctrl-C exits. Panes stack when narrow; below 42x18 a resize hint remains. NO_COLOR is supported. Noninteractive callers use `tui --once` or `tui --json`; otherwise they receive an error without terminal output.
- Native and WASM discovery calls have a five-second budget and cancellation. Terminal cleanup occurs before waiting for a worker. Compilation and blocking host callbacks can exceed the call budget; shutdown latency for those cases is NOT CERTAIN. Ordinary error returns have an RAII cleanup guard and Ratatui installs panic restoration; actual panic/terminal-device failure injection was not performed.
- Rust xtask replaces Python release-note extraction and released-artifact protocol harnesses. Rust integration tests replace the nine installer checks and stdio behavior assertions. All five Python helper implementations were removed after equivalent checks passed. Bootstrap install.sh/install.ps1 remain because end users install before a Rust binary is available.
- Shared executable root ~/rune, managed storage ~/rune/tools, retained-original migration, conflicts, explicit overrides and MCP 2026-07-28 remain intact. A documentation error was corrected: native requests already have a 30-second fallback with no explicit timeout; WASM tool calls have no explicit default timeout. This work did not change those execution defaults.

## Actual validation

- `cargo test --workspace --locked`: 66 tests passed, zero failures. Breakdown: 5 CLI unit, 9 installer, 7 existing CLI output, 5 stdio/TUI process, 5 core unit, 6 annotation compatibility, 21 protocol, 2 security and 6 release utility tests. After the final release-label/extraction assertions, the focused xtask suite of 6 passed again.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: passed. `cargo fmt --all --check`: passed. Workspace build passed.
- Nine Rust installer tests passed on Windows/PowerShell and Git Bash. These preserve canonical/legacy binary, checksum failure/missing checksum, pinned version, replacement, repeated installation, paths with spaces/apostrophes, shared-root Infer sentinels and guarded PATH assertions. The POSIX executable is a stub: this does not verify Linux/macOS native execution.
- Actual Windows ConPTY tests entered and restored the alternate screen after empty inventory and failed discovery, and exited on q. The harness answers the cursor-position query a terminal emulator normally answers. Noninteractive errors/plain/JSON outputs and JSON-only MCP stdout passed.
- A native hanging discovery returned an actionable timeout. An infinite WASM discovery guest was interrupted by its configured budget. Synthetic stdio tests covered successful reads, preserved x-rune-rules and annotation aliases, read-only refusal, timeout, cancellation silence, rejected metadata/version and honest failed discovery.
- Rust conformance ran against released rune-git 0.3.0 (native), rune-time 0.3.0 (WASM), rune-filesystem 0.4.1 (WASM), with the development Kit binary reporting 0.5.0. All 13 Git schemas were preserved; its validator ran and a write was refused. Current time and allowed-root file/directory reads succeeded, traversal and writes were blocked, and bad argument types returned isError. No filesystem 0.4.0 result was reused as 0.4.1 verification.
- A controlled installed inventory using those real artifacts was also inspected through the dashboard: filesystem 0.4.1 exposed 2 read-only tools, git 0.3.0 exposed 1, and time 0.3.0 exposed 2. All were labelled as completed/stopped probes. This was a disposable inventory, not the owner's production installation.
- Existing Rust migration tests passed for retained originals, conflicts, repeated use after updates, simulated interruption and Infer preservation. No migration code or installer default was replaced by this amendment.
- actionlint 1.7.12 passed checks.yml, integrate.yml, release.yml and the existing deploy.yml. ShellCheck/Pyflakes subprocess checks were disabled for this local Windows validator run; actual installer scripts were executed by the Rust suite.
- Two release dry runs were byte-identical, with hashes proving Cargo.toml, Cargo.lock and CHANGELOG.md unchanged. Rust tests verify repeat application, preserved historical entries, matching extracted notes, stale/tampered note detection, bookkeeping/planning-label exclusion and version rules.

Linux/macOS execution and PTY behavior, Windows ARM64, a real hybrid native companion, power-loss migration fault injection, and remote GitHub event/token behavior are NOT CERTAIN and were not run. No remote workflows were dispatched.

## Release flow and owner actions

1. Owner pushes reviewed Conventional Commits to develop. Ubuntu/Windows build, tests, lint and formatting run normally, including on merge and release-preparation commits.
2. Only after checks pass, Actions prepares deterministic manifests/lockfile/changelog plus .release-preparation.json. It updates one release/update-version PR targeting develop, with PR title and automation commit `ci(release): update version`. No releasable changes means no PR; recorded source IDs prevent re-preparation loops.
3. Owner reviews/merges the preparation PR into develop. This alone does not publish. Then promote develop to main using a merge commit or fast-forward, not squash/rebase, so reviewed commit IDs survive. No main commit-message marker is required.
4. Main checks run again; Actions validates prepared history/version/note hash, extracts the exact version section and creates the matching tag/release at that main revision. PAT/App-authenticated release publication triggers deploy.yml for archives/checksums/attestations. Existing releases are not rewritten.

The baseline comes from the highest release tag reachable from main, not develop's nearest tag. This handles main promotion merge tags which are not develop ancestors and prevents reusing a published version. Cocogitto 7.0.0 parses Conventional Commit history; the established feat/fix/perf/breaking rules are applied with Rust semver. Reviewed pending migration notes and explicit Release-note/Migration lines remain public; arbitrary developer body bullets and internal labels do not.

Keep GH_TOKEN configured with repository Contents read/write and Pull requests read/write. It is used for preparation checkout/push/PR and release creation so bot events trigger required checks and artifacts. Protect main/develop with reviews/checks, permit the dedicated bot preparation branch and third-party Actions, and permit the documented promotion merge strategy. No settings or secret values were changed or exposed. Current grants and successful remote event chaining remain NOT CERTAIN until the owner runs the process. See docs/RELEASING.md for details.

## Dry-run example and exact note preview

`cargo xtask release-plan` currently proposes 0.5.0 from the committed history after v0.4.0. Uncommitted dashboard/release-automation work is intentionally not invented as reviewed release prose: it enters the next preparation after the owner commits it. Local artifacts:

- target/release-plan-preview.json
- target/release-changelog-preview.md
- target/release-notes-preview.md

The candidate changelog header is `## 0.5.0 - 2026-10-23`. Exact current candidate release notes follow:

- implement MCP 2026-07-28 discovery and per-request metadata model
- Use MCP 2026-07-28 discovery and per-request version/capability metadata on the public stdio gateway. Clients must replace initialization with server/discover and include metadata on every request. Cancelled requests produce no late response; tool schemas, annotation aliases, read-only restrictions and explicit filesystem roots remain supported.
- Install rune-kit under ~/rune and keep managed plugins, skills and provisioned binaries under ~/rune/tools. Copy legacy managed storage without overwriting conflicts, retain originals for recovery, and preserve custom installation paths and Infer files.
- Read native plugin information through the existing private info operation, retaining the legacy probe for older plugins.

## Remaining non-Rust tooling

Minimal POSIX/PowerShell installers require no end-user Rust toolchain. GitHub Actions YAML, third-party Actions, Git/GitHub CLI, jq and small shell glue handle events/authentication/PRs/artifact packaging. Cocogitto is an established Rust history parser. No repository-owned Python/JavaScript release utility or AI release service remains. actionlint is an optional third-party developer validator written in Go; it was run locally and is not required by the product.

## Suggested owner commit

`feat: add terminal inventory and automate release preparation`

Optional substantive body lines for deterministic release notes:

Release-note: Inspect installed plugin versions, discovered tools, configured roots and actionable errors with rune-kit tui.
Release-note: Prepare release PRs after development checks, then publish reviewed notes and artifacts after owner promotion.

## Files changed

- `.cargo/config.toml`
- `.github/release-commits.tera`
- `.github/workflows/checks.yml`
- `.github/workflows/integrate.yml`
- `.github/workflows/release.yml`
- `AGENTS.md`
- `Cargo.lock`
- `Cargo.toml`
- `cog.toml`
- `crates/cli/Cargo.toml`
- `crates/cli/src/lib.rs`
- `crates/cli/src/tui.rs`
- `crates/cli/tests/installers.rs`
- `crates/cli/tests/stdio_and_tui.rs`
- `crates/cli/tests/support/mod.rs`
- `crates/cli/tests/support/native_fixture.rs`
- `crates/core/src/runtime.rs`
- `crates/core/tests/protocol_tests.rs`
- `docs/MCP-CONTRACT.md`
- `docs/RELEASING.md`
- `docs/TUI.md`
- `README.md`
- `scripts/git_gateway_conformance.py`
- `scripts/release_notes.py`
- `scripts/stdio_acceptance.py`
- `scripts/test_installers.py`
- `scripts/test_release_notes.py`
- `xtask/Cargo.toml`
- `xtask/src/lib.rs`
- `xtask/src/main.rs`
- `xtask/src/release.rs`
- `xtask/src/wire.rs`
- `docs/handoff/release-terminal-report.md`
