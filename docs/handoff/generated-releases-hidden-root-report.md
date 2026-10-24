# Release validation and hidden installation handoff

Date: 2026-10-24. Repository: rune-kit. All changes are local, unstaged and
uncommitted. No publication, tag, push, merge or workflow dispatch was performed.
CHANGELOG.md and the reviewed preparation metadata were not edited.

## Changes

- Main pushes validate prepared state without a specially typed promotion title.
  Valid unpublished versions publish; existing publications validate as no-ops.
  Source ancestry, consumed commit set, package-calculated version, notes hash
  and existing tag notes remain checked. Artifact jobs use the same exact-section
  extraction contract. The existing publication body must match prepared notes.
- git-cliff-core is pinned to 2.14.2. cliff.toml owns conventional parsing,
  semantic bumps, changelogen-style groups, comparison/commit/PR links, breaking
  notes and contributors. Features advance minor; pre-1.0 breaking advances
  minor, post-1.0 breaking advances major; semantic maintenance advances patch.
  Rust retains history selection, synchronization and integrity validation only.
- Installers default to actual-home/.rune/rune-kit[.exe]. Managed plugins, skills,
  metadata and provisioned helpers resolve under actual-home/.rune/tools.
  Explicit installation/data overrides remain supported. No CWD fallback.
- Migration copies known Kit entries from ~/rune/tools and older platform data;
  originals are retained, bytes verified and conflicts preflighted across sources.
  Per-source completion markers make repeats safe. Interrupted copies can resume.
  A completed previous visible-root migration makes its retained platform copy
  historical, preventing stale backups being imported over active data.
  Configured external paths are preserved byte-for-byte, not relocated.
- Windows PATH deduplicates/prepends the new root and keeps the old root for Infer.
  POSIX uses a guarded .profile entry; no-PATH modification remains supported.
  Neither installer removes old Kit/Infer binaries. Infer data remains untouched.
- Self-update verifies an archive digest/checksum and atomically activates the
  invoked executable with Windows rollback; it never updates a second Kit/Infer.
- Attached TUI displays the actual executable and resolved data root. HTTP MCP
  2026-07-28, headless operation, selected-tool restrictions, filesystem ceilings
  and private native/WASM plugin ABI remain unchanged.

## Failure evidence

GitHub run 40061282852 used head ad8c17da49ec8b3f9da4c4e305a683ee0a5f53be.
Both platform checks succeeded; release validation failed. The head subject was
ci(release): update version while the old workflow required
ci(release): publish v0.6.0. Run metadata, commit and workflow established this
mismatch. Authenticated full job logs were unavailable, so their exact stderr is
NOT CERTAIN. The same prepared history now passes local release-check:

```json
{"ready":true,"version":"0.6.0","reason":"Valid unpublished prepared release"}
```

## Actual local checks

Windows host, Rust workspace 0.6.0:

- cargo check --workspace --locked: passed.
- cargo test --workspace --locked: 89 passed, zero failures/ignored.
  Includes migration fresh/visible/platform/repeat/conflict/interruption tests,
  checksummed self-update activation preserving Infer/plugins, explicit overrides,
  ten installer tests, HTTP restrictions/cancellation/concurrency and terminal
  cleanup. PowerShell default-profile/PATH APIs are isolated in the installer
  test; actual user registry/PATH is never modified. POSIX wrapper tests use Git
  Bash on Windows, not a Linux/macOS installation.
- Added PTY executable/data-label assertions and reran the interactive dashboard
  test: passed. Real attached HTTP ping is shown; q restores alternate screen.
  Evidence: target/tui-http-terminal.txt, tui-empty-terminal.txt,
  tui-error-terminal.txt. Actual paths come from current_exe and managed_root.
- cargo clippy --workspace --all-targets --locked -- -D warnings: passed.
- cargo fmt --all; git diff --check: passed.
- actionlint 1.7.12, all four workflow files: passed (shellcheck/pyflakes disabled).
- Five release unit tests pass: package-calculated features/breaking/patch/custom
  maintenance, strongest bump, substantive grouping, exclusions, deterministic
  repeat generation, temporary repeat-apply/history preservation, exact extraction
  and artifact mismatch refusal.
- Two release-plan dry runs were byte-identical: needed=false, version=0.6.0,
  reason=Reviewed changes already prepared. No extra bump or duplicate section.
- Real HTTP conformance passed with debug rune-kit 0.6.0, rune-git-native 0.3.0,
  rune-time WASM 0.3.0 and rune-filesystem WASM 0.4.1. Discovery/list/native Git
  and WASM Time execute; allowed filesystem read/list succeed; outside-root reads
  and forced writes are refused. Evidence: target/hidden-http-conformance.json.
  The filesystem plugin reports outside-root denial inside its content payload;
  the forced write is JSON-RPC -32600. These behaviors were not reclassified.

## Dry-run release preview

Synthetic package fixture, not an edited CHANGELOG or a predicted release:
```markdown
## v0.7.0

[v0.6.0...v0.7.0](https://github.com/shba007/rune-kit/compare/v0.6.0...v0.7.0)

### Enhancements

- **storage:** Use the hidden user installation root ([\#123](https://github.com/shba007/rune-kit/pull/123)) ([0123456](https://github.com/shba007/rune-kit/commit/0123456789abcdef0123456789abcdef01234567))

### Contributors

- Example Contributor

```

The already-reviewed 0.6.0 publication preview remains exactly:

```markdown
- introduce local MCP HTTP server with attached dashboard
```

New development is uncommitted and therefore intentionally absent from release-plan.
The owner should use reviewed semantic subjects describing these changes; CI will
calculate the next version and generate the actual directory migration notes.
Existing reviewed/published notes are not rewritten to change generator styles.

## Files and owner actions

Release: cliff.toml, xtask/Cargo.toml, xtask/src/{main,release}.rs, Cargo.lock,
.github/workflows/{release,deploy}.yml.
Installation/runtime: install.ps1, install.sh, crates/core/src/{paths,package}.rs,
crates/cli/src/{server,tui}.rs, crates/cli/tests/{installers,http_and_tui}.rs.
Guidance: AGENTS.md, README.md, docs/{RELEASING,MCP-CONTRACT,TUI}.md and this report.

Owner reviews/commits/pushes development to develop. After validation, Actions
updates the single release/update-version preparation PR; owner reviews/merges it.
Promote develop to main with a merge commit retaining ancestry. No special title
is needed; merging preparation into develop alone does not publish.

Keep GH_TOKEN Contents write for release-branch pushes/publication, built-in
GITHUB_TOKEN Contents/Pull requests write for preparation PR API operations, and
allow Actions-created PRs. Protect main/develop while permitting owner merge
commits and the dedicated bot branch. GH_TOKEN publication supplies the downstream
release.published event for artifact jobs. These live credentials/settings and
remote publication/event chaining are NOT CERTAIN until owner promotion.

Remaining non-Rust tooling is thin POSIX/PowerShell bootstrap installation, Actions
YAML and GitHub CLI/jq wiring. End users do not need Rust. No AI release service.

NOT CERTAIN: native Linux/macOS execution, live network self-update while replacing
the running Windows process, shell-specific profile loading beyond tested Git
Bash, and remote publication. Self-update validation here uses isolated files and
checksums, not a live GitHub update. POSIX .profile must be loaded by the user's
shell; custom shell profiles may require the documented PATH command. If both old
and new trees differ, migration deliberately requires owner conflict resolution;
Infer migration belongs to Infer and was not attempted. Old Kit copies should
only be removed after validating the hidden installation.

Suggested owner commit subject:
fix: validate prepared releases and migrate Kit to the hidden user root
