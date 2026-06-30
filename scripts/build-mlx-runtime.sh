#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
#
# Build the self-contained MLX runtime tarball for Approach C (download-and-provision).
#
# Produces:
#   dist/meridian-mlx-runtime-<ver>-aarch64.tar.gz   (CPython + venv + agents pkg + launcher)
#   dist/runtime-manifest.json                        (version, url, sha256, size, floors)
#
# The tarball bundles its OWN python-build-standalone CPython, so the customer
# machine needs NO system Python/Node/uv — only Apple Silicon + macOS >= the
# floor below. The ~7 GB model is NOT in here; the server downloads it on first
# request. Layout matches tray/src-tauri/src/mlx_server.rs::resolve_mlx_command
# (extracted to ~/.meridian/runtime/ with `tar --strip-components=1`):
#   meridian-mlx-runtime/bin/python        ← invoked by the tray
#   meridian-mlx-runtime/lib/.../site-packages/{mlx,mlx_lm,outlines,fastapi,agents,…}
#   meridian-mlx-runtime/server.py         ← thin launcher (resolver looks for this)
#
# MUST run on an arm64 runner. The venv is built WITH the bundled interpreter
# (ABI match) — never the runner's system Python. See the Step-2 portability
# notes in the PR. Run from the repo root.
set -euo pipefail

# ── Parameters (env-overridable) ─────────────────────────────────────────────
PY_SERIES="${PY_SERIES:-3.11}"                       # matches requires-python >= 3.11
# MLX/Metal floor — the binding macOS constraint (PBS itself goes back to 11).
# Anything that compiles from source inherits this; prebuilt wheels carry their own.
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-13.5}"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SERVICES_DIR="${REPO_ROOT}/services"
DIST="${REPO_ROOT}/dist"
BUILD="${REPO_ROOT}/.mlx-runtime-build"
RUNTIME_NAME="meridian-mlx-runtime"

VERSION="${VERSION:-$(grep -m1 '^version' "${SERVICES_DIR}/pyproject.toml" | sed -E 's/.*"([^"]+)".*/\1/')}"
TARBALL="${RUNTIME_NAME}-${VERSION}-aarch64.tar.gz"

# Where the asset will live once published (filled by the workflow on a tag).
# RUNTIME_TAG + GITHUB_REPOSITORY → the canonical release-download URL the app fetches.
DOWNLOAD_URL="${RUNTIME_DOWNLOAD_URL:-}"
if [[ -z "${DOWNLOAD_URL}" && -n "${RUNTIME_TAG:-}" && -n "${GITHUB_REPOSITORY:-}" ]]; then
    DOWNLOAD_URL="https://github.com/${GITHUB_REPOSITORY}/releases/download/${RUNTIME_TAG}/${TARBALL}"
fi

echo "→ building ${TARBALL}"
echo "  python series   : ${PY_SERIES}"
echo "  deployment floor: macOS ${MACOSX_DEPLOYMENT_TARGET}"
echo "  arch            : $(uname -m) (must be arm64)"
[[ "$(uname -m)" == "arm64" ]] || { echo "✗ must build on an arm64 runner (got $(uname -m))" >&2; exit 1; }

rm -rf "${BUILD}" && mkdir -p "${BUILD}" "${DIST}"

# ── 1. Fetch a relocatable python-build-standalone CPython (install_only) ─────
# Resolve the install_only aarch64-apple-darwin asset for PY_SERIES from PBS's
# latest release. Pin by setting PBS_ASSET_URL to bypass this lookup.
PBS_ASSET_URL="${PBS_ASSET_URL:-}"
if [[ -z "${PBS_ASSET_URL}" ]]; then
    echo "→ resolving latest python-build-standalone ${PY_SERIES} (aarch64, install_only)"
    # Authenticate the api.github.com lookup when a token is available. The
    # unauthenticated GitHub API limit is 60 req/hr/IP and CI runners share
    # IPs, so this call intermittently 403s (curl exit 56) — it failed the very
    # first auto-publish. A token raises the limit to 1000/hr. The header is
    # added only when a token is present, so local runs still work
    # unauthenticated. (The asset download below is a CDN redirect and is
    # deliberately left unauthenticated.)
    PBS_AUTH=()
    PBS_TOKEN="${GH_TOKEN:-${GITHUB_TOKEN:-}}"
    [[ -n "${PBS_TOKEN}" ]] && PBS_AUTH=(-H "Authorization: Bearer ${PBS_TOKEN}")
    PBS_JSON="$(curl -fsSL ${PBS_AUTH[@]+"${PBS_AUTH[@]}"} https://api.github.com/repos/astral-sh/python-build-standalone/releases/latest)"
    PBS_ASSET_URL="$(printf '%s' "${PBS_JSON}" | python3 -c "
import json, re, sys
rel = json.load(sys.stdin)
pat = re.compile(r'^cpython-${PY_SERIES}\.\d+\+\d+-aarch64-apple-darwin-install_only\.tar\.gz$')
urls = [a['browser_download_url'] for a in rel['assets'] if pat.match(a['name'])]
if not urls:
    sys.exit('no matching PBS asset for ${PY_SERIES} aarch64 install_only')
print(sorted(urls)[-1])
")"
fi
echo "  PBS asset: ${PBS_ASSET_URL}"
curl -fsSL "${PBS_ASSET_URL}" -o "${BUILD}/python.tar.gz"

# PBS install_only extracts to ./python/ — rename it to the runtime dir.
tar -xzf "${BUILD}/python.tar.gz" -C "${BUILD}"
mv "${BUILD}/python" "${BUILD}/${RUNTIME_NAME}"
RT="${BUILD}/${RUNTIME_NAME}"
PYBIN="${RT}/bin/python3"
# The resolver invokes `bin/python` — guarantee that symlink exists.
[[ -e "${RT}/bin/python" ]] || ln -s python3 "${RT}/bin/python"

echo "→ bundled interpreter: $("${PYBIN}" --version)"

# ── 2. Install the MLX stack + agents package WITH the bundled interpreter ────
# ABI match is the whole game: the wheels must be built for THIS CPython, so we
# drive pip through the bundled python — never the runner's system python.
"${PYBIN}" -m pip install --upgrade --no-cache-dir pip wheel >/dev/null
# Non-editable install of meridian-agents[mlx,pm_worklog_update] → the `agents`
# package lands in site-packages, so `import agents` works from any cwd.
"${PYBIN}" -m pip install --no-cache-dir "${SERVICES_DIR}[mlx,pm_worklog_update]"

# ── 3. Thin launcher at the runtime root (what the resolver looks for) ────────
cat > "${RT}/server.py" <<'PYEOF'
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
"""Bundled Meridian MLX runtime launcher (Approach C).

The tray invokes `<runtime>/bin/python <runtime>/server.py --port <p>`. `agents`
is installed as a site-package in this runtime, so the import resolves
regardless of working directory.
"""
from agents.server import main

if __name__ == "__main__":
    main()
PYEOF

# ── 4. Trim build-only cruft (keeps the tarball lean; safe — not imported) ────
find "${RT}" -type d -name "__pycache__" -prune -exec rm -rf {} + 2>/dev/null || true
find "${RT}" -type d -name "tests" -path "*/site-packages/*" -prune -exec rm -rf {} + 2>/dev/null || true

# ── 5. Pack, hash, and emit the manifest ──────────────────────────────────────
echo "→ packing ${TARBALL}"
tar -czf "${DIST}/${TARBALL}" -C "${BUILD}" "${RUNTIME_NAME}"

SHA256="$(shasum -a 256 "${DIST}/${TARBALL}" | awk '{print $1}')"
SIZE="$(stat -f%z "${DIST}/${TARBALL}" 2>/dev/null || stat -c%s "${DIST}/${TARBALL}")"
PY_FULL="$("${PYBIN}" -c 'import platform; print(platform.python_version())')"

cat > "${DIST}/runtime-manifest.json" <<EOF
{
  "version": "${VERSION}",
  "arch": "aarch64",
  "python": "${PY_FULL}",
  "min_macos": "${MACOSX_DEPLOYMENT_TARGET}",
  "tarball": "${TARBALL}",
  "url": "${DOWNLOAD_URL}",
  "sha256": "${SHA256}",
  "size": ${SIZE}
}
EOF

echo "✓ built ${DIST}/${TARBALL}"
echo "  size  : $(( SIZE / 1048576 )) MB"
echo "  sha256: ${SHA256}"
echo "  manifest: ${DIST}/runtime-manifest.json"
