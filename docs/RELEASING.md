# Releasing Rune Kit

Agents implement and test locally, leaving changes unstaged and uncommitted.
Agents never commit, push, merge, tag, publish or dispatch publishing workflows,
and never author/edit CHANGELOG entries. The owner commits development changes;
GitHub Actions generates release files. No AI service generates release prose.

## Owner and workflow sequence

1. Push reviewed conventional commits to develop. Only develop pushes start
   integration/preparation, including merge and release bookkeeping commits.
   PR events and release-branch pushes do not duplicate this pipeline.
2. Ubuntu and Windows validate build, tests, Clippy, formatting and extraction.
3. After success, preparation verifies the tested develop SHA, runs the pinned
   Cocogitto package, synchronizes manifests/lockfile and generates CHANGELOG.
   Generated files and preparation metadata are validated in that same run.
4. One release/update-version branch and PR targeting develop is reused. Its
   title and generated commit remain ci(release): update version. GITHUB_TOKEN
   performs PR API operations; GH_TOKEN pushes the branch. A final remote SHA
   check suppresses stale preparation when new development arrives.
5. Review the generated notes/version/source and successful originating run.
   Merge preparation into develop (merge or squash supported). This alone does
   not publish. Recorded consumed source IDs stop repeat bumps/preparation loops.
6. Promote develop to main using a MERGE COMMIT preserving ancestry; do not
   squash/rebase promotion. There is NO required promotion title or marker.
7. Every main push runs validation and checks prepared version, reachable source
   history, package-calculated version, exact note hash and tag integrity. Valid
   unpublished versions publish. Already published versions are validated no-ops;
   missing/invalid preparation fails clearly. Release bodies extract the exact
   prepared CHANGELOG section, never regenerate prose from history.
8. Publication uses GH_TOKEN so release.published starts artifact builds. Every
   artifact job checks the tag and release body against the same extractor before
   building. Verify all platform assets/checksums after publication.

Run 40061282852 failed after its platform checks: promoted head ad8c17d had
subject ci(release): update version while the old gate demanded
ci(release): publish v0.6.0. The title gate is removed. Local release-check accepts
that prepared 0.6.0 history without publication. Raw authenticated job logs were
unavailable; the mismatch was checked against the run SHA, commit and workflow.

## Package policy and local previews

Pinned Cocogitto =7.0.0 interprets conventional commits, computes versions,
and renders notes through the shared .release/changelog.tera Cocogitto template,
configured in cog.toml. Rust integration
only selects reachable reviewed history, synchronizes files and validates state.
Features advance minor even before 1.0; breaking changes advance minor before
1.0 and major afterwards; fixes and semantic maintenance/custom types advance
patch. Routine release/version/autoformat bookkeeping, invalid messages and
private planning prose are excluded. Notes include scope, linked commits/PRs,
comparison links, breaking changes and contributors where present in history.
No independent handwritten semantic bump or notes renderer remains.

```sh
cargo xtask release-plan
cargo xtask release-example
cargo xtask release-check
cargo xtask release-notes 0.6.0
cargo test --locked -p xtask
```

These commands do not mutate Git or release files. release-plan uses committed
history only. release-example is a deterministic synthetic 0.6.0-to-0.7.0 feature
preview, not a promise about the next release. Repeated preparation of consumed
history does not bump again. --apply is reserved for authorized CI/owner, never
agents on the working repository. GIT_CLIFF_* environment overrides are rejected
so local and CI policy cannot silently diverge. Missing, duplicate, empty or
placeholder version sections fail. Historical sections/tags/releases are retained.
The owner-authorized format migration renders existing local sections through
Cocogitto and refreshes their note receipts. Existing tags and published release
bodies remain immutable. Kit isolates Cocogitto in xtask/generator because its
logging dependencies conflict with the Wasmtime runtime dependency graph.

## GitHub settings and authentication

- Enable Actions to create PRs; preparation needs GITHUB_TOKEN Contents write and
  Pull requests write. PR creation does not need PAT Pull requests write.
- GH_TOKEN must be an owner-managed PAT/App token with repository Contents write
  for release-branch pushes and publication. Never expose its value.
- Permit bot writes to the dedicated branch and owner merge commits on main.
  Require develop integration checks; do not require PR-only checks that this
  intentionally develop-only pipeline does not run.
- Default-token-created events normally suppress downstream workflows. GH_TOKEN
  publication is required for release.published artifact jobs. Artifact event
  chaining remains NOT CERTAIN until the owner runs the promoted workflow.
- Protect develop/main and review prepared source/notes. A racing develop push
  updates the same PR in its next validated run; main validation rejects stale
  preparation containing extra unconsumed releasable history.

Minimal POSIX/PowerShell bootstrap installers remain because end users need no
Rust toolchain. Actions YAML, GitHub CLI and jq remain workflow wiring. Repository
release/protocol/migration validation uses Rust; no Python/JavaScript release
helper is introduced.
