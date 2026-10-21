# Cycle 1 — Kit interoperability correction

Date: 2026-10-21. Implementation commit: 9c4e41a1d06a5f7ea2af70450308a22ba4d4729c. Release commit/tag: 65377ef53abeac9638057073debb00811a2fb99a / v0.3.1.

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

Use the downloaded release rune.exe in `--rune` when validating distribution. Minimum compatible released version: 0.3.1. The official Windows asset was downloaded and passed the same real-plugin acceptance sequence.

## Release status and remaining limits

At verification time, v0.3.0 existed as a remote tag at 3daf070, but no 0.3.0 GitHub Release was published. The earlier Rune Release workflow tagged successfully and failed in Create GitHub Release. Existing tags are preserved; 0.3.1 is the correction version.

The owner repaired GH_TOKEN, retaining all workflow permissions unchanged. The existing release PR #11 was merged into develop, and main was fast-forwarded to release commit 65377ef. The PAT-based release workflow succeeded and automatically triggered binary deployment.

- Release: https://github.com/shba007/rune-kit/releases/tag/v0.3.1
- Release workflow (success): https://github.com/shba007/rune-kit/actions/runs/39647624793
- Binary deployment: https://github.com/shba007/rune-kit/actions/runs/39647652878
- Linux source CI (build, test, lint, format passed): https://github.com/shba007/rune-kit/actions/runs/39645189266

Windows job completed successfully. Downloaded the official ZIP, checked its SHA-256 against GitHub's asset digest, confirmed it contains rune.exe, and executed that downloaded binary:

- Asset: https://github.com/shba007/rune-kit/releases/download/v0.3.1/rune-v0.3.1-x86_64-pc-windows-msvc.zip
- ZIP SHA-256: dc36f6e52ec601f7d7d2b60c9803e546278f4772b8565b3192e2b5155e1b9ed0
- rune.exe SHA-256: a075c5123b4c982e6a0b7cdb6358edeacb534290943dd5465358c6f5c8154dfa
- `rune --version`: `rune 0.3.1`.
- Downloaded-binary integration: 13 normal tools, one read-only tool, all 13 input schemas preserved; validate_commit_message returned valid=true; forced git_commit returned -32600; stderr empty.
- Same rune-git 0.3.0 artifact/hash recorded above. Local evidence: target/cycle1-release-git-result.json and target/release-verification/v0.3.1/verification.json.

No workflow permission changes were applied. No remaining Windows-release blocker. Intel macOS and both Linux assets are also uploaded; Apple Silicon deployment was still running at the last check, so no completion claim is made for that job.

Native cancellation/timeout still kills only the direct process; descendants are not guaranteed to terminate. Synchronous WASM host callbacks can delay cancellation/timeout. No redesign of those limits was attempted. The real native artifact does not accept standalone MCP initialize (`Unknown method: initialize`); gateway initialize succeeds and plugin `info` supplies version metadata. No Tools change was made for that limitation. Full MCP conformance and hybrid host-callback timing remain NOT CERTAIN. The published Windows binary passed the targeted acceptance check above; this is not full MCP conformance certification.
