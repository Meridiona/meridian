#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions
#
# Populate the per-arch npm package (npm/meridian-darwin-arm64) with the
# prebuilt payload, ready for `npm publish`. Run by semantic-release
# (@semantic-release/exec prepareCmd) on a macOS arm64 runner; also runnable
# locally to validate (after building the daemon + UI).
#
#   scripts/package-release.sh <version>
#
# Prerequisites (must already be built):
#   * target/release/meridian   (cargo build --release)
#   * ui/.next/standalone        (npm run build, output:'standalone')
set -euo pipefail

VERSION="${1:?usage: package-release.sh <version>}"
VERSION="${VERSION#v}"

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"

DAEMON_BIN="target/release/meridian"
TRAY_BIN="target/release/meridian-tray"
UI_STANDALONE="ui/.next/standalone"
[[ -x "${DAEMON_BIN}" ]]    || { echo "✗ ${DAEMON_BIN} not found — run: cargo build --release" >&2; exit 1; }
[[ -x "${TRAY_BIN}" ]]      || { echo "✗ ${TRAY_BIN} not found — run: (cd tray && bash create-icons.sh && npm install && npm run tauri build)" >&2; exit 1; }
[[ -d "${UI_STANDALONE}" ]] || { echo "✗ ${UI_STANDALONE} not found — run: (cd ui && npm ci && npm run build)" >&2; exit 1; }

DEST="npm/meridian-darwin-arm64"
echo "→ populating ${DEST} (v${VERSION})"
rm -rf "${DEST}/bin" "${DEST}/ui" "${DEST}/services" "${DEST}/scripts" "${DEST}/.env.example" "${DEST}/VERSION"
mkdir -p "${DEST}/bin" "${DEST}/scripts"

echo "→ daemon binary"
cp "${DAEMON_BIN}" "${DEST}/bin/meridian"
chmod +x "${DEST}/bin/meridian"

echo "→ tray app binary"
cp "${TRAY_BIN}" "${DEST}/bin/meridian-tray"
chmod +x "${DEST}/bin/meridian-tray"

echo "→ Node.js 22 LTS runtime (used here to build the ABI-127 better-sqlite3 addon)"
# Download the official Node 22 LTS binary and verify its SHA-256. We build the
# better-sqlite3 addon shipped in ui.tar.gz against THIS exact Node version, so
# the UI daemon's runtime must match it. The version + SHA are recorded in
# bin/node-runtime.meta (below); install-from-bundle.sh re-downloads this same
# official build on the user's machine — we don't ship the 113 MB binary itself.
_NODE22_VERSION="22.22.3"
_NODE22_SHA="0da7ff74ef8611328c8212f17943368713a2ad953fb7d89a8c8a0eae87c23207"
_node22_tmp="$(mktemp -d)"
_node22_cache_dir="${HOME}/.cache/node22-arm64"
_node22_cache_tgz="${_node22_cache_dir}/node-v${_NODE22_VERSION}-darwin-arm64.tar.gz"
if [[ -f "${_node22_cache_tgz}" ]]; then
    echo "  · using cached Node.js ${_NODE22_VERSION} tarball"
    cp "${_node22_cache_tgz}" "${_node22_tmp}/node22.tar.gz"
else
    curl -fsSL --retry 3 \
        "https://nodejs.org/dist/v${_NODE22_VERSION}/node-v${_NODE22_VERSION}-darwin-arm64.tar.gz" \
        -o "${_node22_tmp}/node22.tar.gz"
    mkdir -p "${_node22_cache_dir}"
    cp "${_node22_tmp}/node22.tar.gz" "${_node22_cache_tgz}"
fi
_actual_sha="$(shasum -a 256 "${_node22_tmp}/node22.tar.gz" | cut -d' ' -f1)"
if [[ "${_actual_sha}" != "${_NODE22_SHA}" ]]; then
    echo "✗ node-v${_NODE22_VERSION} SHA-256 mismatch" >&2
    echo "  expected: ${_NODE22_SHA}" >&2
    echo "  got:      ${_actual_sha}" >&2
    rm -f "${_node22_cache_tgz}"  # evict corrupt cache entry
    rm -rf "${_node22_tmp}"; exit 1
fi
tar -xzf "${_node22_tmp}/node22.tar.gz" -C "${_node22_tmp}"
_NODE22_DIR="${_node22_tmp}/node-v${_NODE22_VERSION}-darwin-arm64"
_NODE22_BIN="${_NODE22_DIR}/bin/node"
echo "  · $("${_NODE22_BIN}" --version) — ABI $("${_NODE22_BIN}" -e 'process.stdout.write(String(process.versions.modules))') ✓"

echo "→ UI (Next.js standalone, packed as a tarball)"
# Always ship ui.tar.gz so fresh installs work even when UI code didn't change.
# The install-layer hash check in meridian-npm-setup.sh preserves the existing
# ui/ dir and skips extraction + UI daemon restart when the tarball is unchanged,
# so update speed is unaffected. Skipping the tarball at package time was a
# ~10 MB optimization that broke first-time installs on any release where the
# UI was not modified (install-from-bundle.sh has no fallback for a missing
# tarball on a machine with no prior ui/ directory).
# WHY a tarball (not a plain ui/ dir): Turbopack's production build references
# serverExternalPackages (better-sqlite3, pino, @opentelemetry/*) via relative
# SYMLINKS under .next/node_modules. npm publish strips symlinks which crash-loops
# the server (vercel/next.js#87737, #93849); tar preserves them intact.
mkdir -p "${DEST}/ui"
_ui_stage="${DEST}/ui"
cp -R "${UI_STANDALONE}/." "${_ui_stage}/"        # cp -R preserves symlinks (BSD/macOS default)
mkdir -p "${_ui_stage}/.next"
cp -R "ui/.next/static" "${_ui_stage}/.next/static"
[[ -d "ui/public" ]] && cp -R "ui/public" "${_ui_stage}/public"
# Swap the better-sqlite3 native addon for the official prebuilt for Node 22 (ABI 127).
# CI runs with Node 24 (ABI 137); the binary from `npm ci && npm run build` only loads
# on Node 24. Download the matching prebuilt directly from GitHub Releases by ABI
# number — no npm lifecycle, no node-gyp, no env var contamination possible.
_bs_version="$("${_NODE22_BIN}" -e "process.stdout.write(require('./ui/node_modules/better-sqlite3/package.json').version)")"
_bs_abi="$("${_NODE22_BIN}" -e 'process.stdout.write(String(process.versions.modules))')"
_bs_url="https://github.com/WiseLibs/better-sqlite3/releases/download/v${_bs_version}/better-sqlite3-v${_bs_version}-node-v${_bs_abi}-darwin-arm64.tar.gz"
_bs_cache_dir="${HOME}/.cache/better-sqlite3-abi${_bs_abi}"
_bs_cached_node="${_bs_cache_dir}/better_sqlite3.node"
_bs_tmp="$(mktemp -d)"

if [[ -f "${_bs_cached_node}" ]]; then
    echo "  · using cached better-sqlite3@${_bs_version} (ABI ${_bs_abi})"
    _bs_node="${_bs_cached_node}"
else
    echo "  · fetching better-sqlite3@${_bs_version} prebuilt for Node 22 (ABI ${_bs_abi})…"
    curl -fsSL --retry 3 "${_bs_url}" | tar -xzf - -C "${_bs_tmp}"
    _bs_node="$(find "${_bs_tmp}" -name "better_sqlite3.node" -path "*/Release/*" 2>/dev/null | head -1)"
    [[ -n "${_bs_node}" && -f "${_bs_node}" ]] || {
        echo "✗ better-sqlite3@${_bs_version} prebuilt for ABI ${_bs_abi} not found at ${_bs_url}" >&2
        rm -rf "${_bs_tmp}" "${_node22_tmp}"; exit 1
    }
    mkdir -p "${_bs_cache_dir}"
    cp "${_bs_node}" "${_bs_cached_node}"
    _bs_node="${_bs_cached_node}"
fi
# Confirm the staged tree has exactly one .node file, then replace it.
_staged_nodes="$(find "${_ui_stage}" -name "better_sqlite3.node" 2>/dev/null)"
_staged_count="$(echo "${_staged_nodes}" | grep -c 'better_sqlite3' 2>/dev/null || echo 0)"
[[ "${_staged_count}" -eq 1 ]] || {
    echo "✗ expected 1 better_sqlite3.node in staged tree, found ${_staged_count}:" >&2
    echo "${_staged_nodes}" >&2
    rm -rf "${_bs_tmp}" "${_node22_tmp}"; exit 1
}
cp "${_bs_node}" "${_staged_nodes}"
rm -rf "${_bs_tmp}"
# Require the .node BINARY directly (absolute path) so Node's dlopen enforces
# NODE_MODULE_VERSION. require(package_dir) is wrong here: better-sqlite3 lazy-
# loads its native addon inside Database() — a bare package require never calls
# dlopen at all, so both the positive and negative checks would always succeed.
_staged_node_abs="${REPO_ROOT}/${_staged_nodes}"
"${_NODE22_BIN}" -e "require('${_staged_node_abs}')" 2>/dev/null || {
    echo "✗ Node 22 failed to load rebuilt better-sqlite3 — aborting" >&2
    rm -rf "${_node22_tmp}"; exit 1
}
echo "  · Node 22 loads ABI 127 binary ✓"
# Negative check: system node (CI = Node 24, ABI 137) must NOT load it.
_sys_node="$(command -v node 2>/dev/null || true)"
if [[ -x "${_sys_node}" ]]; then
    _sys_abi="$("${_sys_node}" -e 'process.stdout.write(String(process.versions.modules))' 2>/dev/null || echo 0)"
    if [[ "${_sys_abi}" != "127" ]]; then
        "${_sys_node}" -e "require('${_staged_node_abs}')" 2>/dev/null && {
            echo "✗ system node (ABI ${_sys_abi}) loaded the ABI 127 binary — swap did not take effect" >&2
            rm -rf "${_node22_tmp}"; exit 1
        }
        echo "  · ABI isolation: $(${_sys_node} --version) (ABI ${_sys_abi}) correctly rejects binary ✓"
    fi
fi
echo "  · better-sqlite3 → Node 22 ABI 127 ($(du -h "${_staged_nodes}" | cut -f1)) ✓"

# Pack (preserving symlinks — no -h) and drop the expanded dir so npm ships only the tarball.
tar -czf "${DEST}/ui.tar.gz" -C "${_ui_stage}" .
rm -rf "${_ui_stage}"
echo "  · ui.tar.gz ($(du -h "${DEST}/ui.tar.gz" | cut -f1), symlinks preserved)"

# Record the exact Node version + SHA-256 that the better-sqlite3 addon in
# ui.tar.gz was built against — but do NOT ship the 113 MB binary through npm
# (the package would exceed the registry's payload limit, E413). Instead
# install-from-bundle.sh downloads this exact official build from nodejs.org,
# verifies this SHA, and caches it under ~/.meridian. ABI stays 127 by
# construction (same source, same version) without bloating the npm tarball.
cat > "${DEST}/bin/node-runtime.meta" <<META
NODE_RUNTIME_VERSION=${_NODE22_VERSION}
NODE_RUNTIME_SHA=${_NODE22_SHA}
META
rm -rf "${_node22_tmp}"
echo "  · node-runtime pinned v${_NODE22_VERSION} (downloaded at install — not bundled)"

echo "→ Python services (source only — venv built from PyPI at install time)"
mkdir -p "${DEST}/services"
tar cf - \
  --exclude='.venv' --exclude='.venv*' --exclude='__pycache__' --exclude='*.pyc' \
  --exclude='logs' --exclude='.hermes' --exclude='.pytest_cache' --exclude='tests/evals/results' \
  --exclude='.claude' --exclude='.claude-flow' --exclude='.git' --exclude='node_modules' \
  --exclude='*.log' --exclude='dist' --exclude='.DS_Store' \
  -C services . | tar xf - -C "${DEST}/services"

echo "→ scripts + plists + CLI"
cp scripts/meridian-cli.sh scripts/install-from-bundle.sh scripts/meridian-npm-setup.sh \
   scripts/bootstrap.sh scripts/ui-start.sh scripts/lib-github-setup.sh \
   scripts/lib-jira-setup.sh "${DEST}/scripts/"
cp scripts/install-daemon.sh scripts/uninstall-daemon.sh \
   scripts/install-ui-daemon.sh scripts/uninstall-ui-daemon.sh \
   scripts/install-screenpipe-daemon.sh scripts/uninstall-screenpipe-daemon.sh \
   scripts/install-a11y-helper-daemon.sh "${DEST}/scripts/"
cp scripts/com.meridiona.daemon.plist \
   scripts/com.meridiona.screenpipe.plist \
   scripts/com.meridiona.a11y-helper.plist \
   scripts/com.meridiona.ui.plist \
   scripts/com.meridiona.tray.plist "${DEST}/scripts/"
cp scripts/install-tray-daemon.sh scripts/uninstall-tray-daemon.sh "${DEST}/scripts/"

# a11y-helper: ship the COMMITTED prebuilt binary byte-for-byte. Never rebuild
# it here — users' Accessibility grants are keyed to its code hash (CDHash);
# changed bytes silently invalidate the grant on every release. Rebuild only
# via scripts/a11y-helper/build.sh when its source changes, and call out the
# permission re-grant in release notes.
mkdir -p "${DEST}/scripts/a11y-helper"
cp scripts/a11y-helper/meridian-a11y-helper "${DEST}/scripts/a11y-helper/"

echo "→ config template + version stamp"
cp .env.example "${DEST}/.env.example"
printf '%s\n' "${VERSION}" > "${DEST}/VERSION"

echo "✓ ${DEST} populated"
du -sh "${DEST}" 2>/dev/null | awk '{print "  payload:", $1}'
