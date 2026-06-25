#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
#
# Populate the per-arch npm package (npm/meridian-darwin-arm64) with the
# prebuilt payload, ready for `npm publish`. Run by semantic-release
# (@semantic-release/exec prepareCmd) on a macOS arm64 runner; also runnable
# locally to validate (after building the daemon + UI).
#
#   scripts/package-release.sh <version>
#
# Prerequisites (must already be built):
#   * target/release/meridian        (cargo build --release)
#   * target/release/meridian-tray   (cd tray && npm run tauri build — embeds the
#                                      dashboard static export into the binary)
set -euo pipefail

VERSION="${1:?usage: package-release.sh <version>}"
VERSION="${VERSION#v}"

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"

DAEMON_BIN="target/release/meridian"
TRAY_BIN="target/release/meridian-tray"
[[ -x "${DAEMON_BIN}" ]] || { echo "✗ ${DAEMON_BIN} not found — run: cargo build --release" >&2; exit 1; }
[[ -x "${TRAY_BIN}" ]]   || { echo "✗ ${TRAY_BIN} not found — run: (cd tray && bash create-icons.sh && npm install && npm run tauri build) — this embeds the dashboard export into the binary" >&2; exit 1; }

DEST="npm/meridian-darwin-arm64"
echo "→ populating ${DEST} (v${VERSION})"
rm -rf "${DEST}/bin" "${DEST}/services" "${DEST}/scripts" "${DEST}/.env.example" "${DEST}/VERSION"
mkdir -p "${DEST}/bin" "${DEST}/scripts"

echo "→ daemon binary"
cp "${DAEMON_BIN}" "${DEST}/bin/meridian"
chmod +x "${DEST}/bin/meridian"

echo "→ tray app binary"
# The dashboard ships INSIDE this binary: `tauri build` embeds the static export
# (ui/out) via generate_context!, so there's no separate ui.tar.gz, no bundled
# Node runtime, and no better-sqlite3 ABI dance any more.
cp "${TRAY_BIN}" "${DEST}/bin/meridian-tray"
chmod +x "${DEST}/bin/meridian-tray"

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
   scripts/bootstrap.sh scripts/lib-github-setup.sh \
   scripts/lib-jira-setup.sh scripts/lib-trello-setup.sh \
   scripts/lib-azure-setup.sh "${DEST}/scripts/"
cp scripts/install-daemon.sh scripts/uninstall-daemon.sh \
   scripts/install-screenpipe-daemon.sh scripts/uninstall-screenpipe-daemon.sh \
   scripts/install-a11y-helper-daemon.sh "${DEST}/scripts/"
# OpenObserve installer + plist: required at runtime by the dashboard's
# "OpenObserve Export" toggle, which bootstraps OO on demand via the tray's
# set_openobserve command → scripts/install-openobserve-daemon.sh. Without these
# in the bundle the toggle errors "OpenObserve installer not found".
cp scripts/install-openobserve-daemon.sh scripts/uninstall-openobserve-daemon.sh \
   "${DEST}/scripts/"
cp scripts/com.meridiona.daemon.plist \
   scripts/com.meridiona.screenpipe.plist \
   scripts/com.meridiona.a11y-helper.plist \
   scripts/com.meridiona.openobserve.plist \
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

# Inject the Trello Power-Up app key into the bundle .env so the daemon can
# run `meridian oauth-login trello` without the user setting anything manually.
# Read from the repo .env (gitignored); skip silently if not set.
_trello_key=""
if [[ -f ".env" ]]; then
    _trello_key="$(grep -E '^TRELLO_APP_KEY=' .env | cut -d= -f2- | tr -d '[:space:]')" || true
fi
if [[ -n "${_trello_key}" ]]; then
    echo "  injecting TRELLO_APP_KEY into bundle .env.example"
    sed -i '' "s|^# TRELLO_APP_KEY=.*|TRELLO_APP_KEY=${_trello_key}|" "${DEST}/.env.example"
else
    echo "  ⚠ TRELLO_APP_KEY not set in .env — Trello OAuth will not work in this bundle"
fi

printf '%s\n' "${VERSION}" > "${DEST}/VERSION"

echo "✓ ${DEST} populated"
du -sh "${DEST}" 2>/dev/null | awk '{print "  payload:", $1}'
