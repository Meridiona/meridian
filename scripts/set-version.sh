#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
#
# Set the product version across every manifest, in lockstep. Called by
# semantic-release (@semantic-release/exec prepareCmd) with the next version.
#
#   scripts/set-version.sh <version>
#
# Updates: Cargo.toml, Cargo.lock, ui/package.json,
# packages/meridian-mcp/package.json, and tray/src-tauri/tauri.conf.json (the
# version the DMG auto-updater compares against — MUST be bumped BEFORE the tray
# build so the packaged .app bakes the release version, not a stale 0.1.0).
# Uses BSD sed (the release runs on macOS).
set -euo pipefail

VER="${1:?usage: set-version.sh <version>}"
VER="${VER#v}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

# TOML: the single top-level `version = "..."` line ([package] / [project]).
sed -i '' -E "s/^version = \"[^\"]*\"/version = \"${VER}\"/" Cargo.toml

# JSON manifests via python (reliable; preserves structure).
python3 - "${VER}" <<'PY'
import json, sys
ver = sys.argv[1]
targets = [
    "ui/package.json",
    "packages/meridian-mcp/package.json",
    # The tray app version — baked into the .app at build time and what
    # tauri-plugin-updater compares against the GitHub latest.json. Top-level
    # "version" key, same shape as a package.json, so the same d["version"] = ver
    # applies. Reformatted to 2-space JSON like the rest (a one-time diff).
    "tray/src-tauri/tauri.conf.json",
]
for path in targets:
    with open(path) as fh:
        d = json.load(fh)
    d["version"] = ver
    with open(path, "w") as fh:
        json.dump(d, fh, indent=2)
        fh.write("\n")

# Sync Cargo.lock's own [[package]] entry for "meridian" so the lockfile never
# drifts from Cargo.toml. Without this the committed lock lags the manifest
# (cargo silently rewrites it on an unlocked build, but a future
# `cargo build --locked` would fail the release). "meridian" is a workspace path
# crate, so only its own version line changes — the dependency graph is untouched,
# leaving rust-cache's restore behaviour the same as the Cargo.toml bump already is.
import re
with open("Cargo.lock") as fh:
    lock = fh.read()
lock, n = re.subn(
    r'(?ms)^(\[\[package\]\]\nname = "meridian"\nversion = ")[^"]*(")',
    lambda m: m.group(1) + ver + m.group(2),
    lock,
    count=1,
)
if n != 1:
    sys.exit(f"set-version: expected exactly one [[package]] meridian in Cargo.lock, patched {n}")
with open("Cargo.lock", "w") as fh:
    fh.write(lock)
print(f"set version {ver} across all manifests + Cargo.lock")
PY
