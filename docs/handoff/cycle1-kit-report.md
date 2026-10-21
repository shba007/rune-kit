# Cycle 1 — Kit interoperability correction

Date: 2026-10-21. Implementation on develop, prepared version 0.3.1.

## Root cause and policy

Rebuilt the original Kit 0.3.0 at 3daf070 and reproduced normal and read-only discovery returning zero tools with `duplicate field destructiveHint` on stderr. Derived Serde aliases treated canonical and legacy spellings as a duplicate field.

The central ToolAnnotations deserializer now accepts either spelling or equal simultaneous pairs for all four hints. Conflicting booleans explicitly reject discovery with both key names; non-booleans, including null, are rejected. Missing facts remain unknown and cannot imply read-only. Unknown unrelated fields are ignored. Native and WASM paths share the decoder. Tool schema JSON remains opaque.

Canonical plus legacy output is retained throughout 0.3.x. Removing legacy output requires a minor-version release and an explicit migration note. Legacy input remains supported. The MCP protocol remains 2025-03-26.

Any plugin tools/list failure now produces JSON-RPC -32000 `Tool discovery failed: plugin '<namespace>': <reason>` instead of a successful empty or partial list. This intentionally fails the aggregate list if even one plugin fails. Existing read-only/allowlist call refusal and timeout/cancel codes remain unchanged.

## Verification

Executed on Windows:

- `cargo test --workspace --locked`: 37 passed, 0 failed (six new annotation tests).
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: passed.
- `cargo fmt --check`: passed.
- `cargo build --workspace --locked`: passed.

Used CARGO_HUSKY_DONT_INSTALL_HOOKS=true. Unit/fixture coverage includes canonical-only, legacy-only, equal pairs, both conflict orders, missing/invalid facts, duplicate identical keys, unknown fields, native and WASM discovery, read-only fail-closed calls, genuine empty discovery versus failure, healthy-plus-failing plugin aggregation, and unchanged inputSchema/x-rune-rules.

Real producer: Rune Tools commit cfba4a140e14189a5ff3cefa54e1db85c3e6fb61; rune-git 0.3.0 (verified through its `info` method). Rebuilt with:

```powershell
cargo build --locked -p rune-git --features native --bin rune-git-native --release --target x86_64-pc-windows-msvc
```

Artifact: D:/Projects/Public/rune/code-tools/target/x86_64-pc-windows-msvc/release/rune-git-native.exe
SHA-256: 2f00f4ac969aa371e16ee6251e0b1973e943a5f323821b5f7c0ff3c61ea9b442.
No Tools source was edited. Other ongoing Tools work was left untouched.

With fixed Kit `rune 0.3.1`: gateway initialization succeeded, normal tools/list returned 13 annotated tools, read-only tools/list returned one tool (`validate_commit_message`), and all 13 input schemas matched the direct producer schemas exactly. Called the validator using its discovered schema with `message: "fix: verify gateway annotation compatibility"`; result contained `valid: true`, no issues, isError=false. Forced `git_commit` in read-only mode returned -32600 before execution. Gateway stderr was empty. Detailed local evidence is target/cycle1-git-result.json.

## Owner commands

From the Kit repository, this performs initialization, both discovery modes, schema comparison, the harmless validator call and the forced-write refusal using the listed names/schema:

```powershell
$env:CARGO_HUSKY_DONT_INSTALL_HOOKS = 'true'
cargo build --workspace --locked
python scripts/git_gateway_conformance.py --rune target/debug/rune.exe --plugin D:/Projects/Public/rune/code-tools/target/x86_64-pc-windows-msvc/release/rune-git-native.exe --output target/cycle1-git-result.json
```

Use the downloaded release rune.exe in `--rune` when validating distribution. Minimum corrected source version is 0.3.1; do not advertise it as an available release until the asset is published and verified.

## Release status and remaining limits

At verification time, v0.3.0 existed as a remote tag at 3daf070, but no 0.3.0 GitHub Release was published. The earlier Rune Release workflow tagged successfully and failed in Create GitHub Release. Existing tags are preserved; 0.3.1 is the correction version.

Release publication, Windows asset download and downloaded `rune --version` are NOT CERTAIN / pending. No corrected release URL or asset is claimed. Proposed release automation repair (repository token plus explicit deployment dispatch) needs owner approval because automatic approval review rejected the added persistent actions:write permission. Alternative: owner repairs GH_TOKEN for the existing workflow. Release owner: repository owner; implementation agent will continue the develop-to-main process once this is resolved.

Native cancellation/timeout still kills only the direct process; descendants are not guaranteed to terminate. Synchronous WASM host callbacks can delay cancellation/timeout. No redesign of those limits was attempted. The real native artifact does not accept standalone MCP initialize (`Unknown method: initialize`); gateway initialize succeeds and plugin `info` supplies version metadata. No Tools change was made for that limitation. Linux CI, full MCP conformance, hybrid host-callback timing and the future published binary are NOT CERTAIN until run.
