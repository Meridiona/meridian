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
print(f"set version {ver} across all manifests")
PY
