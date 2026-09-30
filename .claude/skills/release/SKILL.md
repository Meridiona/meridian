---
name: release
description: "Release the Meridian monorepo. Bumps versions, builds/signs/notarizes the DMG, and publishes via semantic-release."
allowed-tools: Bash, Read, Edit, Grep, Write
---

# Meridian Release Skill

Meridian uses **semantic-release** (not release-please) with an explicit
promotion pipeline:

- `.github/workflows/release-prepare.yml` decides the version and creates an
  immutable tag.
- `.github/workflows/release-build.yml` builds, signs, notarizes, verifies,
  and publishes that exact tag.

**Branch pushes never release.** Merging code and promoting a release are
separate operations. Stable promotion is dispatched from `main`; staging
promotion is dispatched from `pre-main`.

## Components & Version Files

Bumped in lockstep by `scripts/set-version.sh <version>`:

| Component | Version File |
|-----------|--------------|
| Rust daemon | `Cargo.toml` (`version = "X.Y.Z"`), `Cargo.lock` |
| UI | `ui/package.json` |
| MCP server | `packages/meridian-mcp/package.json` |
| Tray app | `tray/src-tauri/tauri.conf.json` |

## Release Workflow

### Production (`main`)
Config: `.releaserc.json`. Conventional commits determine the next stable
version. `release-prepare.yml` runs semantic-release only when a human
explicitly dispatches `channel=stable` from `main`. semantic-release updates
the version/changelog, creates the draft GitHub Release, commits the stable
version bump back to `main`, and pushes the immutable `vX.Y.Z` tag.

The separate `release-build.yml` then builds that tag once on macOS Apple
Silicon and Windows x86_64, signs/notarizes/packages it, composes the updater
manifest, verifies required assets, and publishes the draft. A later merge to
`main` cannot change the source of an in-flight release because the build is
tag-pinned.

### Release candidate (`main`)
Config: `.releaserc.staging.json` (copied over `.releaserc.json` at CI
runtime — never the committed file). The tray builds with the
`tray/src-tauri/tauri.staging.conf.json` overlay (different updater endpoint)
and cuts a prerelease version `X.Y.Z-rc.N`. No RC version-bump commit is written back to `main`. `publishCmd` runs
`scripts/mirror-staging-release.sh <ver>`, which mirrors `latest.json` (and
the DMG) onto a fixed, rolling `updater-staging` GitHub prerelease tag so it
never leaks into production's "latest" pointer.

**Staging has NO `prepareCmd`** — unlike production, its config is a publisher
only. `release-staging.yml` builds the two macOS arches on separate runners
concurrently (`tauri build --no-bundle`), lipos them, then runs
bundle/notarize/package/verify as explicit workflow steps *before* invoking
semantic-release. Everything `prepareCmd` would have done is already on disk
by then. This is what took a staging release from ~43m to ~29m; adding a
`prepareCmd` back would recompile from scratch and undo it. The tradeoff is
that the version gets computed twice (once to stamp the binaries, once to
tag), so the workflow asserts the two agree before publishing.

### 1. Check Current Versions
```bash
grep '^version' Cargo.toml | head -1
grep '"version"' packages/meridian-mcp/package.json | head -1
grep '"version"' tray/src-tauri/tauri.conf.json | head -1
```

### 2. Verify Build & Tests Pass Locally First
```bash
cargo build --release
cargo test
cargo clippy -- -D warnings
cd packages/meridian-mcp && npm run build && cd ../..
```

### 3. Trigger a Release

A merge does **not** publish anything.

Stable promotion:

```bash
gh workflow run release-prepare.yml --ref main -f channel=stable
```

RC promotion:

```bash
gh workflow run release-prepare.yml --ref main -f channel=rc
```

Dry-run either channel by adding `-f dry_run=true`.

The workflow rejects mismatched channel/ref pairs. Once semantic-release creates
the immutable `v*` tag, `release-prepare.yml` dispatches
`release-build.yml` against `main` for cache scope while the build itself
checks out the tag. The exact tagged source is therefore the exact source that
gets signed and published.

### 4. Monitor Build Status

```bash
gh run list --workflow=release-prepare.yml --limit=5
gh run list --workflow=release-build.yml --limit=5
gh run view <RUN_ID> --json status,conclusion,jobs
gh run view <RUN_ID> --log-failed 2>&1 | tail -100
```

## Auto-Update

Client-side update-check/apply logic lives in `tray/src-tauri/src/update.rs`
(`tauri-plugin-updater`). Production is consent-based (in-app banner / tray
menu "Check for Updates…"). A force-update floor exists
(`enforce_minimum_version`, checked 30s after launch then every 6h) sourced
from `tray/minimum-version` (plain `X.Y.Z`) — if that file is absent or
empty, no release carries a `Minimum-Version:` marker and every update stays
consent-based. Only touch `tray/minimum-version` to force-migrate users off a
broken old version; empty/remove it afterward to go back to consent-based.

## Required GitHub Secrets

Signing/notarization needs: `APPLE_SIGNING_IDENTITY`, `APPLE_API_KEY`,
`APPLE_API_ISSUER`, `APPLE_API_KEY_CONTENT`, `APPLE_CERTIFICATE`,
`APPLE_CERTIFICATE_PASSWORD`. Update-manifest signing needs
`TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. Missing
Apple secrets degrade gracefully to ad-hoc signing (which
`verify-release-bundle.sh` then hard-fails on); missing Tauri signing keys
make `package-updater.sh` skip `latest.json` generation (also caught by
`verify-release-bundle.sh`).

## Quick Reference

```bash
# Check what changed since last release
git log --oneline $(git describe --tags --abbrev=0)..HEAD

# List recent releases
gh release list --limit=5

# Re-run failed jobs
gh run rerun <RUN_ID> --failed

# Cancel running build
gh run cancel <RUN_ID>

# Check configured release secrets
gh secret list
```

## Troubleshooting

### Build Failed
```bash
gh run view <RUN_ID> --log-failed 2>&1 | tail -100
```

### SQLX Offline Mode
If Rust build fails with sqlx errors:
```bash
SQLX_OFFLINE=true cargo build --release
```
`.cargo/config.toml` sets this automatically, but double-check it's present.

### MCP Build Failed
```bash
cd packages/meridian-mcp && npm install && npm run build
```

### Updater artifacts missing (`verify-release-bundle.sh` fails)
Usually means `TAURI_SIGNING_PRIVATE_KEY`/`_PASSWORD` is wrong or unset —
`tauri build` silently skips generating `.sig`/`latest.json` in that case
instead of failing. Confirm the secrets with `gh secret list`.
