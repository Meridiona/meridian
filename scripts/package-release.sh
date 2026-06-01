#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions
#
# Assemble a prebuilt release bundle from an already-built repo, into a single
# tarball the installer downloads and unpacks to ~/.meridian/app. Run by
# release.yml on a macOS arm64 runner; also runnable locally to validate.
#
#   scripts/package-release.sh <version> [out_dir]
#
# Prerequisites (must already be built before calling):
#   * target/release/meridian          (cargo build --release)
#   * ui/.next/standalone              (npm run build, with output:'standalone')
#
# Produces: <out_dir>/meridian-<version>-macos-arm64.tar.gz
set -euo pipefail

VERSION="${1:?usage: package-release.sh <version> [out_dir]}"
VERSION="${VERSION#v}"                       # tolerate a leading v
OUT_DIR="${2:-dist}"
ARCH="macos-arm64"
NAME="meridian-${VERSION}-${ARCH}"

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"

DAEMON_BIN="target/release/meridian"
UI_STANDALONE="ui/.next/standalone"
[[ -x "${DAEMON_BIN}" ]]    || { echo "✗ ${DAEMON_BIN} not found — run: cargo build --release" >&2; exit 1; }
[[ -d "${UI_STANDALONE}" ]] || { echo "✗ ${UI_STANDALONE} not found — run: (cd ui && npm ci && npm run build)" >&2; exit 1; }

STAGE="${OUT_DIR}/${NAME}"
rm -rf "${STAGE}"
mkdir -p "${STAGE}/bin" "${STAGE}/ui" "${STAGE}/scripts"

echo "→ daemon binary"
cp "${DAEMON_BIN}" "${STAGE}/bin/meridian"
chmod +x "${STAGE}/bin/meridian"

echo "→ UI (Next.js standalone)"
# standalone/ holds server.js + traced node_modules (incl. the native
# better-sqlite3 .node). static + public are copied in alongside, per Next docs.
cp -R "${UI_STANDALONE}/." "${STAGE}/ui/"
mkdir -p "${STAGE}/ui/.next"
cp -R "ui/.next/static" "${STAGE}/ui/.next/static"
[[ -d "ui/public" ]] && cp -R "ui/public" "${STAGE}/ui/public"

echo "→ Python services (source — venv is created at install)"
# Source only; the venv + MLX wheels are installed on the user's machine.
mkdir -p "${STAGE}/services"
tar cf - \
  --exclude='.venv' --exclude='.venv*' --exclude='__pycache__' --exclude='*.pyc' \
  --exclude='logs' --exclude='.hermes' --exclude='.pytest_cache' --exclude='tests/evals/results' \
  -C services . | tar xf - -C "${STAGE}/services"

echo "→ skills"
[[ -d "services/skills" ]] || echo "  ⚠ services/skills not found (summariser/classifier prompts)"

echo "→ scripts + plists + CLI"
cp scripts/meridian-cli.sh "${STAGE}/scripts/"
cp scripts/install-from-bundle.sh "${STAGE}/scripts/"
cp scripts/install-daemon.sh scripts/uninstall-daemon.sh \
   scripts/install-ui-daemon.sh scripts/uninstall-ui-daemon.sh \
   scripts/install-screenpipe-daemon.sh scripts/uninstall-screenpipe-daemon.sh \
   scripts/com.meridiona.daemon.plist scripts/com.meridiona.screenpipe.plist \
   scripts/com.meridiona.ui.plist "${STAGE}/scripts/" 2>/dev/null || true

echo "→ config template + version stamp"
cp .env.example "${STAGE}/.env.example"
printf '%s\n' "${VERSION}" > "${STAGE}/VERSION"

echo "→ tarball"
mkdir -p "${OUT_DIR}"
tar czf "${OUT_DIR}/${NAME}.tar.gz" -C "${OUT_DIR}" "${NAME}"
rm -rf "${STAGE}"

echo "✓ ${OUT_DIR}/${NAME}.tar.gz"
du -h "${OUT_DIR}/${NAME}.tar.gz" | awk '{print "  size:", $1}'
