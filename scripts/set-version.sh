#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions
#
# Set the product version across every manifest, in lockstep. Called by
# semantic-release (@semantic-release/exec prepareCmd) with the next version.
#
#   scripts/set-version.sh <version>
#
# Updates: Cargo.toml, services/pyproject.toml, ui/package.json,
# packages/meridian-mcp/package.json, the two npm packages, and the main npm
# package's optionalDependencies pin. Uses BSD sed (the release runs on macOS).
set -euo pipefail

VER="${1:?usage: set-version.sh <version>}"
VER="${VER#v}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

# TOML: the single top-level `version = "..."` line ([package] / [project]).
sed -i '' -E "s/^version = \"[^\"]*\"/version = \"${VER}\"/" Cargo.toml
sed -i '' -E "s/^version = \"[^\"]*\"/version = \"${VER}\"/" services/pyproject.toml

# JSON manifests via python (reliable; preserves structure).
python3 - "${VER}" <<'PY'
import json, sys
ver = sys.argv[1]
ARCH_DEP = "@meridiona/meridian-darwin-arm64"
targets = {
    "ui/package.json": False,
    "packages/meridian-mcp/package.json": False,
    "npm/meridian/package.json": True,            # also pin the optionalDependency
    "npm/meridian-darwin-arm64/package.json": False,
}
for path, pin_optdep in targets.items():
    with open(path) as fh:
        d = json.load(fh)
    d["version"] = ver
    if pin_optdep:
        od = d.setdefault("optionalDependencies", {})
        if ARCH_DEP in od:
            od[ARCH_DEP] = ver
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
