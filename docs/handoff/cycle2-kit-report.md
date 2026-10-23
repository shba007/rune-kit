# Cycle 2 verification

Prepared 2026-10-23. Source commit ba7347e; release commit 35afbbc6f7ce4990dd8c582b64d76fcac0ab74d1. Published version 0.4.0. Downloaded Windows version: rune-kit 0.4.0.

39 workspace tests, eight installer tests, locked build/Clippy and formatting passed on Windows. Source CI is https://github.com/shba007/rune-kit/actions/runs/39877604449 . Existing annotation aliases remain emitted.

Released Tools artifacts tested: rune-git-v0.3.0 Windows native ZIP; rune-time-v0.3.0 WASM; rune-filesystem-v0.4.0 WASM. Git validator and both time calls succeeded. Time allowlist get_current_time excluded convert_time, whose forced call returned -32600. Forced git_commit in read-only mode also returned -32600. Native fixture timeout returned -32000 / Tool execution timed out after 1 seconds; actual notifications/cancelled returned no notification reply and the interrupted call returned -32800 / Request cancelled by client with its original ID. Full process-tree and synchronous host callback cancellation remain outside scope.

Studio command: rune-kit run <installed-name-or-artifact> --read-only --allow-tool get_current_time --timeout 2 . Allowlist accepts local or returned fully namespaced names. Initialize protocolVersion=2025-03-26, capabilities={}, clientInfo={name,version}; then notifications/initialized, tools/list, tools/call. Use returned schema and names. Discovery -32000 starts Tool discovery failed:, timeout -32000 starts Tool execution timed out after; other runtime failures share the code. Discard late replies after cancellation by outstanding ID. Legacy executable rune 0.3.1 remains a compatible fallback; 0.4.0 uses rune-kit exclusively. Client configuration invoking rune must be updated. User data stays in rune-kit directories and old unrelated executables are not deleted.

Filesystem installed in isolated APPDATA under target/cycle2/data. Disposable host root target/cycle2/pilot, configured through -p allowed_dir=D:/Projects/Public/rune/code-kit/target/cycle2/pilot . Exact host string maps to the same guest path and reaches plugin config unchanged. Relative arguments hello.txt and . succeeded in normal diagnostic mode. Missing config exposed no host directory; invalid explicit roots rejected; ../outside.txt rejected. The prior implicit working-directory preopen was removed. However read_text_file and list_directory are explicitly readOnlyHint=false in the released artifact, so read-only discovery correctly hides both. Pilot BLOCKED: Tools must correct its annotations; Kit must not infer safer facts. Tools errors can appear as status:error inside JSON text while MCP isError=false. No Tools/Studio edits were made. Native full filesystem sandbox conformance is NOT CERTAIN beyond these targeted checks.

Installer entry points after publication:
irm https://raw.githubusercontent.com/shba007/rune-kit/main/install.ps1 | iex
curl -fsSL https://raw.githubusercontent.com/shba007/rune-kit/main/install.sh | sh
Windows install: %LOCALAPPDATA%/Programs/rune-kit/rune-kit.exe. Existing data: %APPDATA%/rune-kit. POSIX: ~/.local/bin/rune-kit. Version pin: RUNE_KIT_VERSION=v0.4.0. Isolated Windows trial: set RUNE_KIT_INSTALL_DIR to a disposable folder and RUNE_KIT_NO_MODIFY_PATH=1 before running the installer. PowerShell 5.1/7 and Git Bash fixture tests passed; native macOS installer execution remains NOT CERTAIN.


Release: https://github.com/shba007/rune-kit/releases/tag/v0.4.0
Windows: https://github.com/shba007/rune-kit/releases/download/v0.4.0/rune-kit-v0.4.0-x86_64-pc-windows-msvc.zip
ZIP SHA-256: b43dcec6e04807b12060e5e665806e1201467f53219631c12fb2463fc197a853. Verified against the published checksum sidecar. Downloaded binary passed git acceptance and both time operations. PowerShell 5.1 pinned installer trial uses the official release-download base to avoid a temporary unauthenticated GitHub metadata API rate limit; user PATH is unchanged. Normal latest-install metadata access under that rate limit is NOT CERTAIN until the quota resets. No credential access or workflow permission change was used. Filesystem read-only annotation correction is the remaining Tools-owned blocker.
