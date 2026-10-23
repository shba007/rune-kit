# Shared installation and MCP handoff

Prepared locally on 2026-10-23. Proposed release and minimum compatible modern client release: **0.5.0**. No release was published. Changes remain uncommitted and unpushed. No staging, commit, branch, merge, tag, publication or workflow dispatch commands were run by this agent. On resumption, the Git index already contained staged changes; it has been left untouched. Generated Python cache files were removed from the working tree and ignored; the owner should exclude any previously staged cache entries when reviewing the index.

## Changes and behavior

The public stdio gateway implements the applicable MCP 2026-07-28 discovery and per-request metadata model, not the old initialize handshake. It validates version/capability/identity metadata, returns -32022 with supported/requested versions, rejects batches, includes resultType and cache metadata, and suppresses responses after client cancellation. Tool schemas and x-rune-rules are preserved; canonical annotations and supported aliases remain. JSON Schema 2020-12 is the default; external references are offline-only. Invalid arguments produce an isError result without running the tool. Existing read-only, allowlist, rules resources and explicit filesystem-root restrictions remain in force. Native plugin info now uses the existing private info operation with the legacy probe retained. This does not change the native/WASM plugin ABI.

References read: https://modelcontextprotocol.io/specification/2026-07-28/schema , https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning , https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/stdio and https://modelcontextprotocol.io/specification/2026-07-28/server/tools .

The screenshot's initial “Not connected. Kit 0.3.1 or newer is required.” string is Studio's default status in src/components/SettingsTools.vue, rather than a Kit error response. Studio's client still sends initialize for 2025-03-26. The installed executable on this device reports rune-kit 0.4.0. Studio sources were inspected read-only and left untouched. Studio must adopt server/discover and per-request metadata before using 0.5.0. A screenshot alone does not verify what happens after pressing Refresh; that interaction is **NOT CERTAIN**.

## Tests actually run

- Windows x86_64 MSVC development binary: rune-kit 0.5.0. `cargo test --workspace --locked`: **43 passed**, 0 failed. `cargo clippy --workspace --all-targets --locked -- -D warnings`: passed. `cargo fmt --all --check`: passed during final validation. Workspace build passed.
- Offline installer suite: **9 passed**. Windows PowerShell 5.1/available PowerShell, canonical/legacy archives, checksums, pinned versions, existing-executable safeguards, overrides with spaces/apostrophes. Git Bash used POSIX executable stubs to check install/reinstall, a disposable home with spaces, PATH insertion once even when a profile is sourced twice, and Infer coexistence. This is not Linux/macOS execution.
- Release-note extraction suite: **2 passed**, covering exact sections and missing/duplicate/empty/placeholder rejection. Local 0.5.0 preview extracted from CHANGELOG.md.
- Real released native rune-git **0.3.0**: 13 schemas preserved, read-only validator executed successfully, write request refused with -32600. Artifact SHA-256: 6b9af56d1f8daf7c586909bda831dcbd3a7f32fafbe370497def153fb2716f9b.
- Real released WASM rune-time **0.3.0**: read-only get_current_time executed successfully. SHA-256: a80fa8c15bc63df2078ef4fa371395efed2d1bf312889e84530f3a67919c68f7.
- Real released WASM rune-filesystem **0.4.1**: read_text_file and list_directory succeeded under the explicit root, traversal was denied, write_file returned -32600, and invalid argument types returned isError. SHA-256: b513ddd8fc248d436d4b4aa3904a49885239ab75ca8d59550c6fc5b756c2d067. No 0.4.0 filesystem result was reused as verification.
- Synthetic native long-call fixture: --timeout 1 returned -32000; cancellation suppressed its response and the next response matched the subsequent ping. Conflicting plugin annotations caused tools/list -32000 rather than a successful empty list. WASM interruption is covered by the Rust timeout test.
- Self-update: copied the development binary to an isolated “self update home/rune” folder, forced a pinned replacement from published Kit **0.4.0**, verified rune-kit --version reported 0.4.0 and Infer executable/config sentinels were unchanged. This verifies replacement at the shared-root layout, not an unpublished 0.5.0 download.

Linux/macOS native execution, Windows ARM64, real hybrid companion execution, process-tree termination, and power-loss fault injection are **NOT CERTAIN** and were not tested.

## Migration and limits

Installers default to actual user-home/rune, retain explicit RUNE_KIT_INSTALL_DIR, version pinning, checksums and no-PATH options. Executable names stay unchanged. POSIX persists a guarded entry in ~/.profile; shells which do not read that file may need manual PATH configuration. Windows updates user/session PATH without adding the root again. No executable or Infer files are moved out of old locations.

Package metadata, payloads, provisioning, skills and updates share ~/rune/tools. An absolute RUNE_KIT_DATA_DIR selects custom managed storage and does not migrate the default tree into it. Only plugins, skills, _bin, installed.json and installed_skills.json are copied from legacy platform rune-kit storage. Original files remain; differing destination data fails clearly before copying. Files are staged and validated before atomic no-overwrite publication. A completion marker prevents future starts from comparing updated payloads to stale legacy originals. Repeated, conflicting and simulated interrupted copies are covered by Rust tests, including spaces and Infer sentinels. Destination conflicts must be reconciled by the owner; no automatic data merging occurs. Symbolic links are refused. Migration publication requires same-filesystem hard-link support; unsupported filesystems fail while retaining originals. A crash can leave migration staging files; retries ignore them. Real power-loss durability and migration of an entire production user's inventory are **NOT CERTAIN**.

Timeouts remain opt-in for native and WASM tools. Native expiry/cancel kills the supervised process, not a guaranteed descendant process tree. Blocking host callbacks may delay interruption. Cancellation itself produces no public response in this revision. Read-only refusal remains -32600; timeout remains -32000. No optional subscriptions, logging, sampling, elicitation or completions capabilities are advertised.

## Shared wire fixtures

`docs/fixtures/mcp-2026-07-28.json` contains recorded discovery, tools/list, successful call, rejected metadata/version, forbidden tool, cancellation silence, timeout and failed plugin discovery messages with normalized namespaces. Minimum modern Kit release: 0.5.0. These are small examples, not a compatibility framework. A Studio conformance script is welcome once it sends the current protocol format.

## Files changed

- `.github/workflows/integrate.yml`
- `.github/workflows/release.yml`
- `.gitignore`
- `AGENTS.md`
- `CHANGELOG.md`
- `Cargo.toml`
- `Cargo.lock`
- `README.md`
- `crates/cli/src/lib.rs`
- `crates/core/Cargo.toml`
- `crates/core/src/lib.rs`
- `crates/core/src/paths.rs`
- `crates/core/src/package.rs`
- `crates/core/src/provision.rs`
- `crates/core/src/runtime.rs`
- `crates/core/src/protocol.rs`
- `crates/core/tests/protocol_tests.rs`
- `crates/core/tests/annotation_compat_tests.rs`
- `crates/core/tests/fixtures/mcp_native.rs`
- `install.ps1`
- `install.sh`
- `scripts/git_gateway_conformance.py`
- `scripts/stdio_acceptance.py`
- `scripts/test_installers.py`
- `scripts/release_notes.py`
- `scripts/test_release_notes.py`
- `docs/MCP-CONTRACT.md`
- `docs/fixtures/mcp-2026-07-28.json`
- `docs/handoff/shared-installation-mcp-report.md`

## Exact release-note preview

- Use MCP 2026-07-28 discovery and per-request version/capability metadata on the public stdio gateway. Clients must replace initialization with server/discover and include metadata on every request. Cancelled requests produce no late response; tool schemas, annotation aliases, read-only restrictions and explicit filesystem roots remain supported.
- Install rune-kit under ~/rune and keep managed plugins, skills and provisioned binaries under ~/rune/tools. Copy legacy managed storage without overwriting conflicts, retain originals for recovery, and preserve custom installation paths and Infer files.
- Read native plugin information through the existing private info operation, retaining the legacy probe for older plugins.
- Publish release notes from the matching CHANGELOG section and keep CI from creating source commits or release branches.

## Suggested commit subject

`feat: share installation storage and support current MCP discovery`

## Owner release actions

Review the working tree and existing index; exclude generated caches. Commit and push the prepared version, changelog and code through the owner's main/develop process. Update Studio's client to the documented modern wire contract before connecting it to 0.5.0. After approval of the source release, the owner creates and pushes v0.5.0. The release workflow verifies that tag matches the workspace version and extracts the exact matching CHANGELOG section before publishing; --generate-notes is removed. GH_TOKEN remains required for release publication to trigger deploy.yml. The binary workflow publishes release archives/checksums, including rune-kit-v0.5.0-x86_64-pc-windows-msvc.zip containing rune-kit.exe. This future asset is **NOT CERTAIN** until owner-triggered CI succeeds.

CI's automatic formatting commit, version-bump commit and release/update-version PR/branch push were removed. The release workflow no longer creates/pushes tags either; only the owner's tag push triggers publication. No remaining active release workflow creates source commits or pushes branches. No publishing workflow was dispatched during this work.
