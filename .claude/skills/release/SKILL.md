---
name: release
description: "Release the Meridian monorepo. Bumps versions, builds/signs/notarizes the DMG, and publishes via semantic-release."
allowed-tools: Bash, Read, Edit, Grep, Write
---

# Meridian Release Skill

Meridian uses **semantic-release** (not release-please) driven by two GitHub
Actions workflows: `.github/workflows/release.yml` (production, on push to
`main`) and `.github/workflows/release-staging.yml` (staging, on push to
`pre-main` gated by a `[staging-release]` commit-message marker, or manual
dispatch).

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
Config: `.releaserc.json`. Commit-message convention (conventionalcommits
preset): `feat:` → minor, `fix:` → patch, `feat!:`/`BREAKING CHANGE:` → major,
`chore:`/`docs:`/`refactor:` → no bump but included in changelog.

`prepareCmd` runs, in order:
1. `scripts/set-version.sh <ver>` — bump all version files
2. `cargo build --release`
3. UI build (`ui/`), tray build (`tray/`, `tauri build`)
4. `scripts/notarize-dmg.sh <ver>` — notarize + staple the `.dmg` (Tauri only
   signs+notarizes+staples the `.app`, not the `.dmg`)
5. `scripts/package-updater.sh <ver>` — build `latest.json` from the real
   minisign `.sig`, copy the versioned DMG to a stable `Meridian.dmg` name
6. `scripts/verify-release-bundle.sh <ver>` — hard gate: codesign identity,
   Gatekeeper offline acceptance, staple presence, and that the updater
   artifacts (`.app.tar.gz`, `.sig`, `latest.json`) exist and are non-empty
   (Tauri can silently exit 0 with a bad/missing signing key)

Then `@semantic-release/github` publishes `Meridian.dmg`,
`Meridian.app.tar.gz`, `.sig`, and `latest.json` onto a versioned `v<version>`
GitHub Release (becomes GitHub's "latest"), and `@semantic-release/git`
commits the version bump + `CHANGELOG.md` back to `main`
(`chore(release): X.Y.Z [skip ci]`).

### Staging (`pre-main`)
Config: `.releaserc.staging.json` (copied over `.releaserc.json` at CI
runtime — never the committed file). The tray builds with the
`tray/src-tauri/tauri.staging.conf.json` overlay (different updater endpoint)
and cuts a prerelease version `X.Y.Z-staging.N`. No version-bump commit back
to `pre-main`. `publishCmd` runs
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
Production: merge conventional-commit PRs into `main` — `release-prepare.yml`
runs semantic-release automatically on push there, tags `vX.Y.Z`, and the tag
push triggers `release-build.yml`. Staging is manual-only: `gh workflow run
release-prepare.yml --ref pre-main` (or "Run workflow" in the Actions UI with
the branch dropdown set to `pre-main`) — it is no longer cut automatically on
every merge to `pre-main`.

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
