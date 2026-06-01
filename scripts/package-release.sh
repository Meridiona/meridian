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

echo "→ UI (Next.js standalone)"
cp -R "${UI_STANDALONE}/." "${DEST}/ui/"
mkdir -p "${DEST}/ui/.next"
cp -R "ui/.next/static" "${DEST}/ui/.next/static"
[[ -d "ui/public" ]] && cp -R "ui/public" "${DEST}/ui/public"

echo "→ Python services (source — venv is created at install)"
mkdir -p "${DEST}/services"
tar cf - \
  --exclude='.venv' --exclude='.venv*' --exclude='__pycache__' --exclude='*.pyc' \
  --exclude='logs' --exclude='.hermes' --exclude='.pytest_cache' --exclude='tests/evals/results' \
  --exclude='.claude' --exclude='.claude-flow' --exclude='.git' --exclude='node_modules' \
  --exclude='*.log' --exclude='dist' --exclude='.DS_Store' \
  -C services . | tar xf - -C "${DEST}/services"

echo "→ scripts + plists + CLI"
cp scripts/meridian-cli.sh scripts/install-from-bundle.sh scripts/meridian-npm-setup.sh "${DEST}/scripts/"
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
