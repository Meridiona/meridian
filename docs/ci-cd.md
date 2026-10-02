# CI/CD Architecture

This document is the source of truth for Meridian's GitHub Actions, branch
protection, cache topology, and release promotion model.

If this document disagrees with an old comment, plan, staging note, or historical
release document, this document and the active workflow files win.

## Invariants

- `main` is the only development trunk.
- Every code change reaches `main` through a pull request.
- `Fast PR gate` is the required merge status check.
- Merging to `main` never publishes a release.
- Full macOS and Windows workspace validation happens after merge.
- Releases are explicit maintainer promotions.
- Release candidates build the product once.
- Stable promotion reuses the exact signed/notarized/smoked RC binaries.
- Stable promotion must not rebuild product binaries.
- RC versus stable is distribution metadata, not a compile-time product channel.
- PR jobs should consume shared caches where possible; durable caches are written
  from `main` or release jobs, not from PR refs.
- Release-profile Cargo caches stay separate from debug/test CI caches.

## Branch and protection model

### `main`

`main` is the default branch and development trunk.

The active repository ruleset requires:

- pull requests;
- one approving review;
- stale approvals dismissed after a new push;
- review conversations resolved;
- the `Fast PR gate` status check;
- no branch deletion;
- no non-fast-forward updates.

The Fast PR workflow also supports GitHub `merge_group` events so it is safe to
use with Merge Queue. Merge Queue is a repository setting/policy, not a reason to
duplicate full platform CI on every PR.

There is no `pre-main` branch, no `pre-main` ruleset, and no staging branch in
the release architecture.

## Pull-request pipeline

Workflow: `.github/workflows/pr-fast.yml`

Purpose: give developers merge-blocking feedback quickly. It is intentionally
not a release qualification workflow.

The workflow path-classifies the diff and runs only relevant jobs:

| Surface | Fast PR behavior |
|---|---|
| Rust formatting | `cargo fmt --check` |
| `meridian-core` | package-scoped clippy + tests on Linux |
| `meridian-oauth` | package-scoped clippy + tests on Linux |
| daemon/root crate | daemon library/CLI clippy + library tests on Linux |
| Tauri tray Rust | tray library clippy on macOS |
| UI | Bun tests + TypeScript check + static Next.js build |
| installer/package files | macOS install-package regression tests |
| migrations | append-only migration guard |
| release/license policy | release-shape + screenpipe license-pin checks |

All selected results aggregate into the single required status context:

`Fast PR gate`

The fast workflow does not:

- build installers;
- sign or notarize;
- run the full workspace on macOS and Windows;
- publish releases.

Those belong to post-merge validation and release qualification.

## Post-merge pipeline

Workflow: `.github/workflows/ci.yml`

Trigger: push to `main`.

Purpose: exhaustive platform confidence after integration, without making every
PR wait for the slowest platform build.

For Rust changes it runs the full workspace on:

- macOS Apple Silicon;
- Windows x86_64.

Clippy and test legs run independently so one does not serialize behind the
other. The workflow also runs UI, migration, installer, and release-policy
validation when their paths require it.

A failure here means `main` needs to be fixed before cutting the next release.
It does not retroactively change the PR gate into a release gate.

## Main candidate manifests

Every successful post-merge CI run on `main` uploads a small metadata artifact
named `candidate-<full-sha>`. It records the exact integrated commit, CI run, and
associated merge PRs. It does not contain product binaries and does not publish a
GitHub Release.

An RC must name that exact candidate SHA. `release-prepare.yml` verifies that:

- the candidate is the current `main` HEAD;
- a successful post-merge `CI` run exists for that exact SHA; and
- the matching unexpired candidate manifest artifact exists.

The current-HEAD restriction is deliberate. Semantic version analysis runs on
current `main`; tagging an older SHA while analyzing newer commits could assign a
version that does not describe the source being released. If current `main` is not
releasable, fix/revert/feature-flag it and let CI create a new candidate.

## Release model

### Overview

The release pipeline is three explicit workflows:

- `.github/workflows/release-prepare.yml`
- `.github/workflows/release-build.yml`
- `.github/workflows/promote-rc.yml`

The key rule is:

```text
PR -> main -> post-merge validation -> RC build/proof -> stable promotion
```

### Release candidate

Trigger:

```bash
gh workflow run release-prepare.yml \
  --repo Meridiona/meridian \
  --ref main \
  -f candidate_sha=<full-successful-main-sha>
```

`release-prepare.yml` uses semantic-release in version-analysis mode to derive
the next stable base version, then creates an immutable `vX.Y.Z-rc.N` tag.

It dispatches `release-build.yml` against `main` for cache scope while passing
the immutable tag as an input. The build workflow checks out the tag itself.

The RC build:

1. validates the tag/release;
2. builds macOS aarch64 and Windows x86_64 concurrently;
3. signs the updater payloads;
4. signs and notarizes macOS;
5. packages Windows;
6. composes updater manifests;
7. smoke-tests the exact uploaded macOS and Windows artifacts;
8. attaches `smoke-macos.ok` and `smoke-windows.ok`;
9. publishes the prerelease only after both exact-artifact proofs pass.

RC binaries are stamped with the final `X.Y.Z` application version. They are
not compiled as a special "staging" binary.

After automated exact-artifact smoke passes, a maintainer manually validates
the published prerelease before promoting it. Promotion is intentionally a
separate action; publishing an RC never makes it stable automatically.

### Stable promotion

Trigger:

```bash
gh workflow run promote-rc.yml \
  --repo Meridiona/meridian \
  --ref main \
  -f rc_tag=vX.Y.Z-rc.N
```

Stable promotion is metadata/artifact promotion only.

It verifies the named candidate is a published prerelease at the resolved RC
tag commit, validates allowed source drift, and refuses to overwrite an existing
stable tag or release. It downloads every RC asset and checks each byte against
GitHub's SHA-256 digest, verifies both smoke proofs bind the RC tag and commit to
the same successful `Release (build & publish)` run, and validates that the
complete updater manifest exactly matches its signed platform fragments.

Only after those checks does it create the stable tag/draft release and upload
the staged asset set. Product binaries, signatures, and smoke proofs must remain
byte-for-byte identical. The three updater JSON assets are rewritten only to
retarget download URLs from the RC tag to the stable tag; `latest.json` also
carries forward any currently armed `Minimum-Version` policy. The workflow
checks every uploaded stable asset's SHA-256 and size while the release is still
a draft, publishes it, then verifies the published release and `latest` channel.
Any missing asset, malformed proof, unexpected JSON asset, metadata mismatch, or
integrity mismatch fails closed.

If a run stops after creating the stable draft but before uploading anything, a
retry may resume only that exact empty, non-prerelease draft at the proven RC
commit. Any existing tag, published release, different target, or non-empty
draft remains a hard stop; promotion never overwrites release state.

It does **not** run `release-build.yml` and does **not** compile, sign, notarize,
or package the product again.

Therefore the DMG, app archive, Windows installer, signatures, and smoke proofs
on stable must be byte-for-byte identical to the RC. Only updater JSON metadata
is expected to differ.

The `v1.92.2-rc.3 -> v1.92.2` promotion proved this path in production: stable
promotion completed in well under five minutes with identical product-artifact
digests.

After the stable release is verified, maintainers may optionally run **Set
minimum supported version**. That policy step is deliberately separate from
promotion and never changes product binaries.

## Updater model

The packaged app uses the stable endpoint configured in
`tray/src-tauri/tauri.conf.json`:

`https://github.com/Meridiona/meridian/releases/latest/download/latest.json`

There is no rolling staging updater channel.

A user installing an RC is testing an exact candidate artifact. RC-to-RC
auto-update behavior is not a release invariant.

## Minimum supported version policy

Minimum-version enforcement is intentionally separate from building or promoting
a release. First publish and validate stable. If the fleet later needs a forced
upgrade, run `.github/workflows/minimum-version.yml` from GitHub Actions.

The workflow accepts either `set` with a published stable `X.Y.Z`, or `clear`.
`set` verifies that `vX.Y.Z` is a published non-prerelease and is not newer than
the current latest stable release. It then edits only that latest stable release's
`latest.json` asset, adding `Minimum-Version: X.Y.Z` to its notes. No binary is
rebuilt, resigned, retagged, or republished.

The running DMG app checks that line shortly after launch and periodically.
Clients below the floor automatically install the latest stable update and
relaunch. Clients at or above the floor retain normal update behavior.

Stable promotion carries an already-armed floor forward from the previous latest
stable manifest, so the policy survives future stable releases until explicitly
changed or cleared. RC manifests do not carry the fleet floor.

The old source-controlled `tray/minimum-version` mechanism is retired.

`tray/minimum-version` remains the emergency force-update floor. Empty or absent
means normal consent-based updating.

## Product smoke tests

Release qualification uses the artifacts that were actually uploaded, not a
fresh local rebuild.

- macOS smoke downloads/mounts the candidate artifact and verifies the signed
  packaged application.
- Windows smoke downloads the exact NSIS installer, installs it, starts the
  packaged product, validates daemon lifecycle/health, and exercises uninstall.
- Successful release smokes create immutable proof assets:
  `smoke-macos.ok` and `smoke-windows.ok`.

The manual product-smoke workflows are also useful for validating already
published artifacts without recompiling them.

## Cache architecture

GitHub Actions cache storage is repository-wide and finite. Cache duplication
can evict the caches that save the most build time, so cache topology is part of
CI architecture.

### Rust cache families

| Family | Writer | Readers | Purpose |
|---|---|---|---|
| `ci-macos-silicon` | post-merge macOS CI on `main` | macOS CI + Fast PR tray | debug/check/test dependencies |
| `ci-windows` | post-merge Windows CI on `main` | Windows CI | debug/check/test dependencies |
| `macos-release-aarch64-apple-darwin` | RC release build on `main` cache scope | later macOS release builds | release-profile dependencies |
| `windows-release-x86_64-pc-windows-msvc` | RC release build on `main` cache scope | later Windows release builds | release-profile dependencies |

Do not merge CI and release cache families. Cargo debug/test and release
artifacts are not interchangeable, and macOS/Windows artifacts are not
cross-platform.

Fast PR Linux jobs deliberately do not maintain separate durable target-cache
families. The old `pr-fast-core`, `pr-fast-oauth`, `pr-fast-daemon`, and
`pr-fast-macos-tray` lineages are retired.

### UI caches

Main CI owns the durable Next.js build-cache lineage. PR UI jobs restore from
that lineage but do not create a separate per-PR cache family.

`actions/setup-node` manages npm download caches independently.

### Cache maintenance

- `.github/workflows/cache-audit.yml` reports repository usage, family sizes,
  ref scopes, and largest entries.
- `.github/workflows/cache-prune.yml` removes superseded generations,
  unreachable tag/closed-PR entries, obsolete cache lineages, and legacy
  sccache objects.
- Cache pruning is housekeeping, not a merge-critical build.
- Never add a new large cache family without identifying its writer, readers,
  scope, expected size, and why an existing family cannot be reused.

## Change-routing rules

When changing CI/release files, preserve these boundaries:

- `pr-fast.yml`: pre-merge fast feedback only.
- `ci.yml`: exhaustive post-merge validation.
- `release-prepare.yml`: RC version analysis, tag creation, and build dispatch.
- `release-build.yml`: RC product build/sign/notarize/package/smoke/publish.
- `promote-rc.yml`: build-free, exact-artifact RC-to-stable promotion.
- product-smoke workflows/scripts: test existing artifacts; do not rebuild.
- cache workflows: observe/prune cache storage; do not compile the product.

A workflow-only PR should not force an expensive product compile unless the
workflow being changed genuinely requires that validation.

## When code architecture becomes a CI concern

Do not split Rust crates merely to improve a dashboard number.

First verify the slow job had a compatible cache hit. If a warm Fast PR tray job
still exceeds the latency target, inspect the actual Cargo dependency graph and
workspace recompilation.

The first known architectural pressure point is that `meridian-tray` depends
on the root `meridian` crate, which can broaden its compile graph. If measured
warm builds remain slow, identify the exact symbols crossing that boundary and
move only independently testable contracts/services into a narrower shared
crate. Avoid microcrate decomposition without measurements.

## Operational commands

Recent CI:

```bash
gh run list --repo Meridiona/meridian --limit 20
```

Release runs:

```bash
gh run list --repo Meridiona/meridian --workflow=release-prepare.yml --limit 10
gh run list --repo Meridiona/meridian --workflow=release-build.yml --limit 10
```

Inspect a run:

```bash
gh run view <RUN_ID> --repo Meridiona/meridian --json status,conclusion,jobs
gh run view <RUN_ID> --repo Meridiona/meridian --log-failed
```

Manual cache audit/prune:

```bash
gh workflow run cache-audit.yml --repo Meridiona/meridian --ref main
gh workflow run cache-prune.yml --repo Meridiona/meridian --ref main
```

## Historical architecture

Old `pre-main`, `*-staging.*` release trains, rolling staging updater tags,
automatic branch-push releases, cache-warm builds, and stable rebuilds are
retired architecture.

Do not reintroduce them from old commit history, stale comments, or external
notes.
