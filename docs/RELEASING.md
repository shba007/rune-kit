# Owner-reviewed release flow

Agents edit/test locally and leave changes unstaged/uncommitted. The human owner
commits and pushes development changes, reviews/merges preparation PRs and
promotes develop to main. Only GitHub Actions creates release preparation commits,
pushes its branch, publishes tags/releases and builds/uploads artifacts. No AI
service generates notes. Product source/runtime behavior is never changed by a
release-preparation commit beyond the version/lock/changelog metadata.

## Events and owner steps

1. Owner pushes reviewed semantic commits to **develop**. Only develop pushes
   trigger Rust CI, including merge and release bookkeeping commits. PR events
   and release-branch pushes do not duplicate integration/preparation.
2. Ubuntu and Windows run build, workspace tests, Clippy and formatting. Current
   changelog extraction is checked. Preparation runs only after these pass.
3. Preparation checks out develop and verifies HEAD equals the tested event SHA.
   Rust xtask reads reviewed history with git-conventional, computes semver,
   writes workspace/lock versions, substantive changelog and the consumed source
   set in .release-preparation.json. Generated files are validated in this run
   with locked metadata, release-check and release utility tests.
4. A final remote develop SHA check suppresses stale preparation if development
   arrived during the job; its newer validated run prepares the updated release.
   One release/update-version branch/PR targets develop. Title and generated
   commit are **ci(release): update version**. GITHUB_TOKEN handles PR API calls;
   GH_TOKEN pushes the branch. No releasable changes produces no new PR. Re-runs
   reuse the source set and fixed branch, not duplicate versions/sections/PRs.
5. Owner reviews notes/version/diff and successful checks in the originating
   preparation run, then merges the PR into develop (merge or squash supported).
   PR-created workflow runs are intentionally unnecessary. Merging into develop
   alone does not publish. Routine preparation commits are tested normally and
   the recorded source set prevents another release for the same changes.
6. Owner opens develop → main promotion PR and chooses **Create a merge commit**,
   preserving ancestry. Do NOT squash or rebase promotion. Set main merge title
   exactly **ci(release): publish vX.Y.Z**, matching the prepared workspace version.
7. Main's release action runs checks, verifies that marker, merge ancestry and
   prepared source/version/note hash, extracts the exact CHANGELOG section and
   creates the matching tag/release using GH_TOKEN. It does not generate fresh
   release prose. Existing published releases remain unchanged; safe retries
   validate prepared metadata before publishing a missing release.
8. The PAT/App-authenticated release.published event starts deploy.yml, builds
   tagged platform archives and uploads assets/checksums/attestations. Owner
   verifies all target jobs and the actual installable downloads.

A last-moment develop push can race any pre-push check; the next validated run
updates the one PR. Publication still refuses stale preparation against promoted
history. Protect main and develop and review the prepared source/notes before
promotion; no merge into main should bypass the owner step.

## Rules and previews

All valid semantic types are eligible, including docs/refactor/style/test/build/
ci/chore/revert/custom. feat/fix/perf keep the repository's existing minor/patch
rules; breaking changes before 1.0 advance minor, after 1.0 advance major.
Maintenance uses patch. Routine release/version/autoformat bookkeeping, invalid
messages and private/placeholder planning prose are excluded. Public commit
summaries plus explicit Release-note:/Migration: body lines supply deterministic
notes; reviewed unpublished migration prose is retained. Published sections,
commits/tags/releases are preserved. The greatest published tag reachable from
main supplies the baseline; source IDs and exact note hashes prevent loops.

For this HTTP transport migration, use a breaking conventional subject, e.g.
**feat!: serve local MCP over HTTP with an attached dashboard**. The planned
minimum compatible release is **0.6.0** after owner-driven preparation; 0.5.0
remains the current manifest until CI calculates the next release. The new
unpublished changelog section records the migration for preparation to retain.

Read-only local commands:

```sh
cargo xtask release-plan
cargo xtask release-example
cargo xtask release-notes 0.6.0
cargo test --locked -p xtask
```

release-plan reflects committed history only. An unchanged published tree needs
no release even when local edits exist. release-example prints a deterministic
synthetic breaking migration preview without changing Git/files. --apply is
reserved for authorized CI/owner, never used by an agent on the working repo.
Missing/duplicate/mismatched/empty/placeholder sections fail extraction. Rust
uses git-conventional, semver and toml_edit; the old Cocogitto JSON template was
removed. Cocogitto commit-message validation may still be used by owner hooks.

## Required GitHub configuration

- GITHUB_TOKEN job permissions: Contents write and Pull requests write for
  preparation; allow Actions to create PRs in repository/organization settings.
- GH_TOKEN secret: owner-managed PAT/App with repository Contents write for
  release/update-version branch pushes and release publication. PR API calls do
  not depend on the PAT having Pull requests write. Keep token values private.
- Permit third-party Actions and bot writes on the dedicated release branch.
  Verify the originating preparation run and develop integration checks before
  promotion. Do not require a pull_request-only check on release/update-version:
  those events intentionally do not run CI. Permit merge commits on main;
  squash-only promotion breaks consumed history.
- GitHub normally suppresses downstream workflows from GITHUB_TOKEN-created
  events. Publishing with GH_TOKEN preserves release.published artifact dispatch.
  No dispatch workaround or agent-owned publishing step is used. Actual remote
  event chaining is NOT CERTAIN until the owner commits/pushes/promotes this flow.

Minimal POSIX/PowerShell installers remain necessary before Rust is installed;
end users need no Rust toolchain. Actions YAML, GitHub CLI and jq remain workflow
wiring; new release/protocol validation utilities are Rust. Shared binary/data
roots and migration logic are unchanged.

Authentication/event references checked for this implementation:
- [Create Pull Request v8 token/branch-token inputs](https://github.com/peter-evans/create-pull-request/blob/v8/action.yml)
- [GitHub workflow event/token behavior](https://docs.github.com/en/actions/how-tos/write-workflows/choose-when-workflows-run/trigger-a-workflow)
These confirm the static event model; they do not verify this repository's secrets
or a live publication run.
