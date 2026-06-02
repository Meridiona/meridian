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
mkdir -p "${DEST}/bin" "${DEST}/ui" "${DEST}/scripts"

echo "→ daemon binary"
cp "${DAEMON_BIN}" "${DEST}/bin/meridian"
chmod +x "${DEST}/bin/meridian"

echo "→ UI (Next.js standalone, packed as a tarball)"
# Assemble the runnable standalone tree (server + static + public), then pack it
# into a single tarball. WHY a tarball and not a plain ui/ dir: the Turbopack
# production build references serverExternalPackages (better-sqlite3, pino,
# @opentelemetry/*) via relative SYMLINKS under .next/node_modules. `npm publish`
# strips symlinks, which crash-loops the standalone server on the user's machine
# (vercel/next.js#87737, #93849). tar preserves symlinks and npm ships the .tgz
# as one opaque file, so the exact built tree round-trips intact;
# install-from-bundle.sh extracts it back into ~/.meridian/app/ui. This is what
# lets the production build stay on Turbopack despite our npm distribution.
_ui_stage="${DEST}/ui"
cp -R "${UI_STANDALONE}/." "${_ui_stage}/"        # cp -R preserves symlinks (BSD/macOS default)
mkdir -p "${_ui_stage}/.next"
cp -R "ui/.next/static" "${_ui_stage}/.next/static"
[[ -d "ui/public" ]] && cp -R "ui/public" "${_ui_stage}/public"
# Pack (preserving symlinks — no -h) and drop the expanded dir so npm ships only the tarball.
tar -czf "${DEST}/ui.tar.gz" -C "${_ui_stage}" .
rm -rf "${_ui_stage}"
echo "  · ui.tar.gz ($(du -h "${DEST}/ui.tar.gz" | cut -f1), symlinks preserved)"

echo "→ Python services (source + pre-built site-packages)"
mkdir -p "${DEST}/services"
tar cf - \
  --exclude='.venv' --exclude='.venv*' --exclude='__pycache__' --exclude='*.pyc' \
  --exclude='logs' --exclude='.hermes' --exclude='.pytest_cache' --exclude='tests/evals/results' \
  --exclude='.claude' --exclude='.claude-flow' --exclude='.git' --exclude='node_modules' \
  --exclude='*.log' --exclude='dist' --exclude='.DS_Store' \
  -C services . | tar xf - -C "${DEST}/services"

echo "→ Python venv (pre-built site-packages — avoids PyPI at install time)"
# uv must be available on the CI runner. pip3 install is blocked on macOS 26
# by PEP 668 (externally-managed-environment). Install via Homebrew instead.
command -v uv >/dev/null 2>&1 || brew install uv
# Build the venv from the committed uv.lock (exact pinned set, no resolution).
# Pin Python 3.11 explicitly: the macos-26 runner defaults to Python 3.14 which
# produces cpython-314-darwin.so extensions that Python 3.11 on user machines
# cannot load (ImportError at startup). uv installs Python 3.11 automatically.
uv sync --project services --extra mlx --frozen --python 3.11
# Validate the venv was actually built with Python 3.11. If CI's uv defaults
# change or --python is removed, this guard catches it before shipping a broken tarball.
_py_dir="$(ls -d services/.venv/lib/python* 2>/dev/null | head -1 | xargs basename 2>/dev/null || true)"
if [[ -z "${_py_dir}" ]]; then
    echo "✗ could not find python lib dir under services/.venv/lib/" >&2; exit 1
fi
if [[ "${_py_dir}" != "python3.11" ]]; then
    echo "✗ venv was built with ${_py_dir} but must be python3.11 — check --python in uv sync" >&2; exit 1
fi
# Pack site-packages only — NOT pyvenv.cfg or bin/ (those are path-specific and
# are re-created at install time by `uv venv`). The tarball is platform-specific:
# it contains the arm64 native extensions (mlx-metal etc.) built on this runner.
echo "  · packing services/.venv/lib/${_py_dir}/site-packages → ${DEST}/services-venv.tar.gz"
tar -czf "${DEST}/services-venv.tar.gz" \
    -C "services/.venv/lib/${_py_dir}/site-packages" .
echo "  · $(du -sh "${DEST}/services-venv.tar.gz" | cut -f1) compressed"

echo "→ scripts + plists + CLI"
cp scripts/meridian-cli.sh scripts/install-from-bundle.sh scripts/meridian-npm-setup.sh \
   scripts/bootstrap.sh "${DEST}/scripts/"
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
