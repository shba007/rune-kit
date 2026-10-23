# Release preparation and publication

Agents implement and test locally, leaving changes unstaged and uncommitted. The owner commits/pushes code and reviews/merges release PRs. Only GitHub Actions performs automated preparation commits, branch pushes, tagging, publication and artifact uploads. No AI service is used.

## Event sequence

1. The owner pushes reviewed Conventional Commits to `develop`. Feature PRs may be squash-merged with a meaningful Conventional Commit subject.
2. Rust CI runs the reusable checks on Ubuntu and Windows: build, all Rust tests (including installer/protocol/TUI checks), Clippy and formatting. Merge commits and `ci(release):` commits are tested normally.
3. Only after success, preparation checks out the exact tested development revision. A newer development revision causes the stale preparation job to stop. Cocogitto 7.0.0 parses reviewed history; Rust semver applies the established feat/fix/perf/breaking rules. Rust xtask writes Cargo.toml, Cargo.lock, the new CHANGELOG section and `.release-preparation.json`.
4. The single `release/update-version` branch/PR targets `develop`; both PR title and automation commit are **`ci(release): update version`**. Re-runs update this branch instead of creating another PR. No releasable changes means no preparation PR. Its bot-created PR runs the same validation checks, plus preparation validation.
5. The owner reviews/merges that PR into `develop`. This alone **does not publish**. Its source-commit set and note hash prevent another preparation from the same reviewed changes. New feature/fix/performance commits can update an outstanding unpublished release without repeatedly incrementing its version.
6. The owner then opens/merges a `develop` → `main` promotion PR. **Use a merge commit or fast-forward, never squash or rebase this promotion**: preparation records the reviewed source commit IDs. The preparation PR itself may use a merge commit or squash merge with its prescribed title. No main commit-message marker is required; validation uses the prepared metadata.
7. A push to `main` runs the same checks. Publication verifies source history, workspace version and the prepared note hash, extracts the exact matching CHANGELOG section, then creates `v<version>` and the GitHub release at that main revision. Existing releases are left unchanged. A tagged-but-unpublished retry validates its prepared notes before publishing.
8. The PAT/App-authenticated `release: published` event starts `deploy.yml`, which builds the tagged source and uploads platform archives, checksums and attestations. Main publication never regenerates substantive prose from Git history.

## Rules and idempotence

The existing `cog.toml` commit categories and release rules remain the authority: feat increments minor, fix/perf increments patch; breaking changes follow Cocogitto's pre-1.0 minor-version convention. Other commit types alone do not open a release PR. Internal planning labels and routine version/release bookkeeping are excluded from public entries. Reviewed summaries (or explicit `Release-note:` / `Migration:` lines) supply deterministic prose. Important migration entries from an already reviewed, unpublished CHANGELOG section are retained. Published version sections are never regenerated.

The latest version tag reachable from main is the baseline, even when main created a merge commit that is not an ancestor of develop. The exclusion range removes already released source commits; the Rust semver crate advances that published baseline using the documented rules. Never calculate from develop’s nearest ancestor tag. The source-commit set is sorted, so merge ordering cannot increment a release. The date comes from reviewed history, not the runner clock. `.release-preparation.json` records the published baseline, target version, included source IDs and exact note hash. Don't manually edit generated notes after preparation; make the substantive correction in reviewed development history and let preparation update the PR. Do not run mutating `cog bump`: it would create a local commit/tag outside this flow. The release tools invoke only non-mutating `cog changelog`; semantic version calculation uses the Rust semver crate.

## Local previews (no Git mutation or publication)

```sh
cargo xtask release-plan
cargo xtask release-notes 0.5.0
cargo test -p xtask
```

`release-plan` prints version, exact candidate section/notes, source IDs and whether preparation is needed. It excludes uncommitted work. `release-plan --apply` writes preparation files only; it is used by Actions, never commits/tags/pushes, and is tested against disposable directories. Publication uses `cargo xtask release-check` and `cargo xtask release-notes VERSION --output PATH`. Missing, duplicate, mismatched, empty or placeholder notes fail clearly. Install Cocogitto 7.0.0 for history previews; it is not needed by end users.

## Authentication and repository settings

- Retain the existing **GH_TOKEN** secret. Use an owner-managed PAT or an appropriately refreshed GitHub App installation token with repository **Contents: read/write** and **Pull requests: read/write**. Checkout/preparation and PR creation use that token; release creation also uses it. Do not substitute the built-in token silently.
- GitHub suppresses new workflow runs for most events created with GITHUB_TOKEN. The existing PAT/App setup is required so preparation PRs trigger required checks and releases trigger artifact publication. See [GitHub's event-triggering rules](https://docs.github.com/en/actions/how-tos/writing-workflows/choosing-when-your-workflow-runs/triggering-a-workflow).
- Actions must be enabled, with the referenced third-party Actions permitted by repository/organization policy. Rulesets must permit the bot to push `release/update-version` and open PRs; protect `main` and `develop` with the reusable Rust build checks and owner review. If organization policy additionally restricts Actions PR creation, the owner must allow it. No agent changes these settings or inspects secret values.
- Permit merge commits for develop-to-main promotion; a squash-only main policy is incompatible with recorded source IDs. Do not auto-merge preparation or promotion PRs without owner review.
- Workflow permissions are `contents: read` for checks; preparation uses `contents: write` and `pull-requests: write`; publication uses `contents: write`. Existing artifact attestation permissions remain in deploy.yml. Required token grants and successful remote event chaining are **NOT CERTAIN** until the owner runs the flow; local tests do not publish or dispatch it.

## Rust tooling and bootstrap exceptions

Installer tests are `crates/cli/tests/installers.rs`; stdio/TUI process tests are `crates/cli/tests/stdio_and_tui.rs`; released-artifact checks use:

```sh
cargo xtask conformance /path/rune-kit /path/rune-git-native /path/rune-time.wasm /path/rune-filesystem.wasm
```

Release utilities and harnesses are Rust. Python helpers are removed after equivalent assertions pass. Minimal install.sh and install.ps1 remain because installation happens before a Rust binary exists; no Rust toolchain is needed by end users. Actions YAML, GitHub CLI, jq and small shell commands glue together third-party Actions and events. Cocogitto is an established Rust release tool; it is not an AI service. Git stores source history and authenticates CI pushes. No separate JavaScript release utility is introduced.
