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
UI_STANDALONE="ui/.next/standalone"
[[ -x "${DAEMON_BIN}" ]]    || { echo "✗ ${DAEMON_BIN} not found — run: cargo build --release" >&2; exit 1; }
[[ -d "${UI_STANDALONE}" ]] || { echo "✗ ${UI_STANDALONE} not found — run: (cd ui && npm ci && npm run build)" >&2; exit 1; }

DEST="npm/meridian-darwin-arm64"
echo "→ populating ${DEST} (v${VERSION})"
rm -rf "${DEST}/bin" "${DEST}/ui" "${DEST}/services" "${DEST}/scripts" "${DEST}/.env.example" "${DEST}/VERSION"
mkdir -p "${DEST}/bin" "${DEST}/scripts"

# Compute _prev_tag once — used by both the UI and venv conditional-ship logic.
_prev_tag="$(git describe --tags --abbrev=0 HEAD^ 2>/dev/null || true)"

echo "→ daemon binary"
cp "${DAEMON_BIN}" "${DEST}/bin/meridian"
chmod +x "${DEST}/bin/meridian"

echo "→ Node.js 22 LTS runtime (bundled — UI daemon uses this, not system node)"
# Download the official Node 22 LTS binary and verify its SHA-256.
# This binary is later exec'd by ui-start.sh (via MERIDIAN_NODE_BIN in the
# launchd plist). Because the better-sqlite3 addon is also built against this
# exact Node version below, ABI is always correct regardless of what node
# the user has installed.
_NODE22_VERSION="22.22.3"
_NODE22_SHA="0da7ff74ef8611328c8212f17943368713a2ad953fb7d89a8c8a0eae87c23207"
_node22_tmp="$(mktemp -d)"
curl -fsSL --retry 3 \
    "https://nodejs.org/dist/v${_NODE22_VERSION}/node-v${_NODE22_VERSION}-darwin-arm64.tar.gz" \
    -o "${_node22_tmp}/node22.tar.gz"
_actual_sha="$(shasum -a 256 "${_node22_tmp}/node22.tar.gz" | cut -d' ' -f1)"
if [[ "${_actual_sha}" != "${_NODE22_SHA}" ]]; then
    echo "✗ node-v${_NODE22_VERSION} SHA-256 mismatch" >&2
    echo "  expected: ${_NODE22_SHA}" >&2
    echo "  got:      ${_actual_sha}" >&2
    rm -rf "${_node22_tmp}"; exit 1
fi
tar -xzf "${_node22_tmp}/node22.tar.gz" -C "${_node22_tmp}"
_NODE22_DIR="${_node22_tmp}/node-v${_NODE22_VERSION}-darwin-arm64"
_NODE22_BIN="${_NODE22_DIR}/bin/node"
echo "  · $("${_NODE22_BIN}" --version) — ABI $("${_NODE22_BIN}" -e 'process.stdout.write(String(process.versions.modules))') ✓"

echo "→ UI (Next.js standalone, packed as a tarball)"
# Only rebuild and ship the UI tarball when the dashboard has actually changed
# since the previous release tag. Most releases change only the Rust binary or
# Python services; skipping the tarball saves ~10 MB per update download for
# those users. When absent, meridian-npm-setup.sh preserves the existing ui/ dir
# and install-from-bundle.sh skips extraction + UI daemon restart entirely.
# WHY a tarball (not a plain ui/ dir): Turbopack's production build references
# serverExternalPackages (better-sqlite3, pino, @opentelemetry/*) via relative
# SYMLINKS under .next/node_modules. npm publish strips symlinks which crash-loops
# the server (vercel/next.js#87737, #93849); tar preserves them intact.
_ui_changed=1
if [[ -n "${_prev_tag}" ]]; then
    if git diff --quiet "${_prev_tag}" HEAD -- ui/ scripts/package-release.sh 2>/dev/null; then
        _ui_changed=0
    fi
fi
if [[ "${_ui_changed}" -eq 1 ]]; then
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
    echo "  · fetching better-sqlite3@${_bs_version} prebuilt for Node 22 (ABI ${_bs_abi})…"
    _bs_tmp="$(mktemp -d)"
    curl -fsSL --retry 3 "${_bs_url}" | tar -xzf - -C "${_bs_tmp}"
    _bs_node="$(find "${_bs_tmp}" -name "better_sqlite3.node" -path "*/Release/*" 2>/dev/null | head -1)"
    [[ -n "${_bs_node}" && -f "${_bs_node}" ]] || {
        echo "✗ better-sqlite3@${_bs_version} prebuilt for ABI ${_bs_abi} not found at ${_bs_url}" >&2
        rm -rf "${_bs_tmp}" "${_node22_tmp}"; exit 1
    }
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
else
    echo "  UI unchanged since ${_prev_tag} — skipping UI tarball (~10 MB saved per update)"
fi

# Copy the bundled node binary into the package — always, regardless of whether
# the UI tarball was skipped. Every release must include bin/node-runtime so
# ui-start.sh never falls back to a user-installed node with a different ABI.
cp "${_NODE22_BIN}" "${DEST}/bin/node-runtime"
chmod +x "${DEST}/bin/node-runtime"
rm -rf "${_node22_tmp}"
echo "  · node-runtime bundled ($(du -h "${DEST}/bin/node-runtime" | cut -f1))"

echo "→ Python services (source + pre-built site-packages)"
mkdir -p "${DEST}/services"
tar cf - \
  --exclude='.venv' --exclude='.venv*' --exclude='__pycache__' --exclude='*.pyc' \
  --exclude='logs' --exclude='.hermes' --exclude='.pytest_cache' --exclude='tests/evals/results' \
  --exclude='.claude' --exclude='.claude-flow' --exclude='.git' --exclude='node_modules' \
  --exclude='*.log' --exclude='dist' --exclude='.DS_Store' \
  -C services . | tar xf - -C "${DEST}/services"

echo "→ Python venv (pre-built site-packages)"
# Only rebuild and ship the venv tarball when the venv's contents could have
# changed since the previous git tag — i.e. when services/uv.lock OR this script
# changed. The extras installed (--extra mlx --extra pm_worklog_update) live in
# THIS script, so a change to which extras ship must force a rebuild even when
# uv.lock is untouched — otherwise an extras fix would ship a stale tarball.
# Shipping on every release would force all users to download ~160 MB even when
# only the Rust binary or UI changed; when neither input changed the installer
# falls back to `uv sync --frozen` (a no-op from the warm uv cache on updates).
_lock_changed=1
if [[ -n "${_prev_tag}" ]]; then
    if git diff --quiet "${_prev_tag}" HEAD -- services/uv.lock scripts/package-release.sh 2>/dev/null; then
        _lock_changed=0
    fi
fi

# On macOS 26+, always ship the tarball regardless of lock changes.
# apple-fm-sdk (Apple Intelligence) is source-only on PyPI — it can only be
# compiled here (CI has full Xcode). If we skip the tarball, the installer falls
# back to `uv sync --frozen` which removes apple-fm-sdk (not in the lockfile),
# breaking Apple Intelligence for every update where only non-Python files changed.
_ci_macos_major="$(sw_vers -productVersion 2>/dev/null | cut -d. -f1)"
if [[ "${_ci_macos_major:-0}" -ge 26 ]]; then
    _lock_changed=1
fi

if [[ "${_lock_changed}" -eq 1 ]]; then
    echo "  uv.lock changed since ${_prev_tag:-beginning} — building and shipping venv tarball (~160 MB)"
    # uv must be available on the CI runner. pip3 install is blocked on macOS 26
    # by PEP 668 (externally-managed-environment). Install via Homebrew instead.
    command -v uv >/dev/null 2>&1 || brew install uv
    # Pin Python 3.11: macos-26 defaults to Python 3.14 which produces
    # cpython-314-darwin.so that Python 3.11 on user machines cannot load.
    # Both extras: mlx (classifier) AND pm_worklog_update (agno) — the shipped
    # MLX server serves /synthesise_worklog too, which imports agno; without it
    # worklog synthesis 500s with ModuleNotFoundError on every install.
    uv sync --project services --extra mlx --extra pm_worklog_update --frozen --python 3.11
    # Validate the venv is actually Python 3.11.
    _py_dir="$(ls -d services/.venv/lib/python* 2>/dev/null | head -1 | xargs basename 2>/dev/null || true)"
    if [[ -z "${_py_dir}" ]]; then
        echo "✗ could not find python lib dir under services/.venv/lib/" >&2; exit 1
    fi
    if [[ "${_py_dir}" != "python3.11" ]]; then
        echo "✗ venv was built with ${_py_dir} but must be python3.11" >&2; exit 1
    fi
    # On macOS 26+, install apple-fm-sdk into the venv so end users get Apple
    # Intelligence without needing Xcode. The CI runner (macos-26) has Xcode;
    # the package is source-only (no PyPI wheels) so must be compiled here.
    _pkg_macos_major="$(sw_vers -productVersion 2>/dev/null | cut -d. -f1)"
    if [[ "${_pkg_macos_major:-0}" -ge 26 ]]; then
        echo "  macOS ${_pkg_macos_major}: compiling apple-fm-sdk for Apple Intelligence…"
        uv pip install --python "services/.venv/bin/python" "apple-fm-sdk"
        echo "  · apple-fm-sdk compiled and included in bundle"
    fi
    tar -czf "${DEST}/services-venv.tar.gz" \
        -C "services/.venv/lib/${_py_dir}/site-packages" .
    echo "  · $(du -sh "${DEST}/services-venv.tar.gz" | cut -f1) compressed — included in bundle"
else
    echo "  uv.lock unchanged since ${_prev_tag} — skipping venv tarball (~160 MB saved per update)"
    echo "  Installers fall back to uv sync (3ms no-op from warm cache on updates)"
fi

echo "→ scripts + plists + CLI"
cp scripts/meridian-cli.sh scripts/install-from-bundle.sh scripts/meridian-npm-setup.sh \
   scripts/bootstrap.sh scripts/ui-start.sh "${DEST}/scripts/"
cp scripts/install-daemon.sh scripts/uninstall-daemon.sh \
   scripts/install-ui-daemon.sh scripts/uninstall-ui-daemon.sh \
   scripts/install-screenpipe-daemon.sh scripts/uninstall-screenpipe-daemon.sh \
   scripts/com.meridiona.daemon.plist scripts/com.meridiona.screenpipe.plist \
   scripts/com.meridiona.ui.plist "${DEST}/scripts/" 2>/dev/null || true

echo "→ config template + version stamp"
cp .env.example "${DEST}/.env.example"
printf '%s\n' "${VERSION}" > "${DEST}/VERSION"

echo "✓ ${DEST} populated"
du -sh "${DEST}" 2>/dev/null | awk '{print "  payload:", $1}'
