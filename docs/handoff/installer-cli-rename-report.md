# Installer and CLI naming update

Prepared on develop, 2026-10-22. Not pushed or published; source workspace version remains 0.3.1. Command rename is a breaking change recommended for release 0.4.0.

The sole Cargo binary is now rune-kit (rune-kit.exe on Windows). Clap help/version, CLI messages, current README instructions, agent documentation, MCP contract, conformance-script examples and future release asset packaging use rune-kit. No rune alias is produced. Historical release evidence and native plugin names such as rune-git-native remain intact.

Added root install.ps1 and install.sh, following rune-infer's per-user install approach. Windows defaults to %LOCALAPPDATA%/Programs/rune-kit and adds user/session PATH; Linux/macOS default to ~/.local/bin/rune-kit and print the PATH command if needed. No administrator privileges or Rust toolchain are needed for release installation. Pinning, local/release-URL mirrors, checksums and reinstalling are supported. Windows has an explicit no-PATH-change option. Checksum verification and candidate version validation happen before an existing executable is replaced. Archive extraction selects only the expected executable. Staged replacement preserves the old executable on a failed move.

Future release jobs produce rune-kit-<tag>-<target> archives and adjacent .sha256 files, with unchanged workflow permissions. The Windows installer accepts current legacy rune archives and verifies GitHub's digest when there is no checksum sidecar. Older binaries installed as rune-kit.exe still report their historical rune display name. POSIX installs require checksum sidecars, introduced by the renamed release.

Self-update reads both new rune-kit and legacy rune binaries from archives. Existing plugin, skill, runtime and provisioner data already use the rune-kit data directory (%APPDATA%/rune-kit on Windows); no data migration is needed. Users must change MCP client commands to rune-kit. Existing unrelated executables are left in place for manual removal after migration.

Executed checks before the final documentation/CI wiring:

- cargo test --workspace --locked: 38 passed.
- cargo clippy --workspace --all-targets --locked -- -D warnings: passed.
- cargo fmt --check and cargo build --workspace --locked: passed.
- python scripts/test_installers.py --binary target/debug/rune-kit.exe: 8 passed, including PowerShell 5.1/7, install/reinstall, legacy archive, checksum mismatch/missing checksum preserving prior data, and POSIX quoted paths.
- Real download: PowerShell 5.1 installed published v0.3.1 into target/installer-live-check as rune-kit.exe, using the release digest, with PATH modification disabled; reports historical rune 0.3.1 as expected.
- Rebuilt rune-kit CLI against the real rune-git 0.3.0 artifact: 13 normal tools, one read-only validator, all schemas preserved, validator valid=true, forced write refused with -32600.

The POSIX fixture tests ran in Git Bash on Windows. Native Linux/macOS execution and persistent user PATH updates are NOT CERTAIN; the tests intentionally avoid changing the user's environment. A POSIX installer smoke step is prepared in the existing Linux CI workflow but has not run on GitHub yet. No release or main-branch change was performed.
