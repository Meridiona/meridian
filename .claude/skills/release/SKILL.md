---
name: release
description: "Operate Meridian's explicit RC-to-stable release promotion pipeline."
allowed-tools: Bash, Read, Edit, Grep, Write
---

# Meridian Release Skill

Read `docs/ci-cd.md` first. It is the canonical description of branch, CI,
cache, smoke-test, and release architecture.

Meridian uses semantic-release for **version analysis**, but production release
publication is an explicit two-stage promotion pipeline:

- `.github/workflows/release-prepare.yml` decides/validates the release and
  creates tags/releases.
- `.github/workflows/release-build.yml` builds, signs, notarizes, packages,
  smoke-tests, and publishes an **RC**.
- Stable promotion reuses the exact proven RC artifacts and does not dispatch a
  product build.

A push or merge to `main` never publishes a release.

## Release invariants

- `main` is the only development trunk.
- RCs are immutable candidate tags: `vX.Y.Z-rc.N`.
- RC binaries are built with final application version `X.Y.Z`; there is no
  compile-time staging channel.
- macOS and Windows exact-artifact smokes must pass before an RC is published.
- A proven RC carries `smoke-macos.ok` and `smoke-windows.ok`.
- Stable promotion must name a proven RC.
- Stable promotion creates `vX.Y.Z` at the RC's source commit.
- Stable product artifacts must be byte-for-byte identical to the RC.
- Only updater/release JSON metadata is expected to change during stable
  promotion.
- Never reintroduce a rolling staging updater tag or a stable rebuild.

## Version files

During an RC product build, `scripts/set-version.sh <X.Y.Z>` updates the
checkout used for compilation so all shipped components agree on the final
application version:

| Component | Version file |
|---|---|
| Rust daemon/workspace | `Cargo.toml`, `Cargo.lock` |
| UI | `ui/package.json` |
| MCP server | `packages/meridian-mcp/package.json` |
| Tray | `tray/src-tauri/tauri.conf.json` |

These build-worktree changes are not a separate release-branch model.

## Cut an RC

First make sure the intended source is on `main` and post-merge validation is
healthy.

Trigger:

```bash
gh workflow run release-prepare.yml \
  --repo Meridiona/meridian \
  --ref main \
  -f channel=rc
```

`release-prepare.yml` runs semantic-release in analysis/dry-run mode to derive
the next stable base version, chooses the next RC ordinal, creates
`vX.Y.Z-rc.N`, and dispatches `release-build.yml`.

The build workflow is dispatched on `main` for default-branch cache access,
but checks out the immutable tag supplied as input. Do not change this cache/tag
split casually.

The RC workflow:

1. validates the candidate tag and draft release;
2. builds macOS aarch64 and Windows x86_64 in parallel;
3. sets the product version to final `X.Y.Z`;
4. signs updater payloads;
5. signs/notarizes macOS;
6. packages Windows;
7. composes updater manifests;
8. uploads candidate assets;
9. runs exact-artifact macOS and Windows smokes;
10. uploads proof assets;
11. publishes the RC only after both proofs exist.

## Promote a proven RC to stable

Trigger:

```bash
gh workflow run release-prepare.yml \
  --repo Meridiona/meridian \
  --ref main \
  -f channel=stable \
  -f candidate_tag=vX.Y.Z-rc.N
```

Stable promotion is intentionally fast and build-free. It validates the
candidate and allowed source drift, creates the stable tag/release at the RC
source commit, downloads the RC assets, rewrites updater-manifest download URLs
from the RC tag to the stable tag, uploads the unchanged binary artifacts plus
rewritten manifests, and publishes stable.

Do not run `release-build.yml` for stable.

## Verify artifact identity

For a promoted stable release, compare the GitHub asset SHA-256 digests of:

- `Meridian-aarch64.app.tar.gz`
- `Meridian-aarch64.app.tar.gz.sig`
- `Meridian-aarch64.dmg`
- `Meridian-x86_64-setup.exe`
- `Meridian-x86_64-setup.exe.sig`
- `smoke-macos.ok`
- `smoke-windows.ok`

They must match the candidate RC. `latest.json` and per-platform updater JSON
files are allowed to differ because stable promotion rewrites tag URLs and
stable metadata.

## Monitor release runs

```bash
gh run list --repo Meridiona/meridian --workflow=release-prepare.yml --limit=10
gh run list --repo Meridiona/meridian --workflow=release-build.yml --limit=10

gh run view <RUN_ID> --repo Meridiona/meridian --json status,conclusion,jobs
gh run view <RUN_ID> --repo Meridiona/meridian --log-failed
```

Re-run only failed jobs when appropriate:

```bash
gh run rerun <RUN_ID> --repo Meridiona/meridian --failed
```

## Auto-update

Client update logic lives in `tray/src-tauri/src/update.rs`.

Stable packaged apps read:

`https://github.com/Meridiona/meridian/releases/latest/download/latest.json`

There is no staging updater channel.

`tray/minimum-version` is the emergency force-update floor. Empty/absent means
normal consent-based updating. Set it only when older shipped versions must be
forced forward; clear it again after the fleet has moved.

## Product smokes

Release qualification always tests uploaded artifacts, not a fresh rebuild.

- macOS verifies the exact packaged signed application.
- Windows installs the exact NSIS package, starts the product, validates daemon
  lifecycle/health, and uninstalls it.
- proof assets are part of the promotion contract.

Manual product-smoke workflows may be used to re-check already published
artifacts without rebuilding.

## Required secrets

Signing/notarization uses `APPLE_SIGNING_IDENTITY`, `APPLE_API_KEY`,
`APPLE_API_ISSUER`, `APPLE_API_KEY_CONTENT`, `APPLE_CERTIFICATE`, and
`APPLE_CERTIFICATE_PASSWORD`.

Updater signing uses `TAURI_SIGNING_PRIVATE_KEY` and
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.

OAuth/telemetry build secrets used by the product remain defined by the release
workflow. Never print or copy secret values into release logs.

## Troubleshooting

### RC build is unexpectedly cold

Inspect the `Swatinem/rust-cache` restore output. The intended release cache
families are documented in `docs/ci-cd.md`. Run the cache audit before
inventing a new cache key:

```bash
gh workflow run cache-audit.yml --repo Meridiona/meridian --ref main
```

### Stable promotion tries to build

That is an architecture regression. Stable promotion must not dispatch
`release-build.yml`.

### Candidate rejected because source drift is unsafe

Cut a new RC from current `main`. Only narrowly defined CI/release-control
changes are allowed after an RC; product/source drift requires a new candidate.

### Updater artifacts are missing

Check updater-signing secrets and the packaging/verification steps in
`release-build.yml`. Do not publish stable without a proven RC asset set.

## Historical warning

Ignore old references to `pre-main`, `release-staging.yml`, rolling
`updater-staging`, automatic branch-push releases, or rebuilding stable.
Those are retired designs.
