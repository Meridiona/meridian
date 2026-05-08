---
name: release
description: "Release the Meridian monorepo. Bumps versions, verifies builds, and triggers GitHub Actions via release-please."
allowed-tools: Bash, Read, Edit, Grep, Write
---

# Meridian Release Skill

## Components & Version Files

| Component | Version File | Workflow |
|-----------|--------------|----------|
| Rust daemon | `Cargo.toml` (`version = "X.Y.Z"`) | `release-please.yml` |
| MCP server | `packages/meridian-mcp/package.json` (`"version": "X.Y.Z"`) | `release-please.yml` |

Meridian uses **release-please** for automated changelog and version management.
Config: `release-please-config.json`, manifest: `.release-please-manifest.json`.

## Release Workflow

### 1. Check Current Versions
```bash
echo "=== Daemon ===" && grep '^version' Cargo.toml | head -1
echo "=== MCP ===" && grep '"version"' packages/meridian-mcp/package.json | head -1
```

### 2. Verify Build & Tests Pass
```bash
cargo build --release
cargo test
cargo clippy -- -D warnings
cd packages/meridian-mcp && npm run build && cd ../..
```

### 3. How release-please Works
- Merge PRs with conventional commits (`feat:`, `fix:`, `chore:`) into `main`
- release-please bot opens a "Release PR" automatically
- Merging the Release PR bumps versions, tags the release, and triggers the release workflow

### 4. Trigger Build
```bash
# After merging the release-please PR, monitor the CI run
gh run list --workflow=release-please.yml --limit=5
gh run view <RUN_ID> --json status,conclusion,jobs
```

### 5. Monitor Build Status
```bash
gh run view <RUN_ID> --log-failed 2>&1 | tail -100
```

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

## Commit Conventions
release-please uses conventional commits to determine version bumps:
- `feat:` → minor bump
- `fix:` → patch bump
- `feat!:` or `BREAKING CHANGE:` → major bump
- `chore:`, `docs:`, `refactor:` → no bump (but included in changelog)
