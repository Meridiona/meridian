#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
# Install the meridian Next.js dashboard as a launchd LaunchAgent under the
# current user. Serves on http://localhost:3939. Built artifact must exist
# at ui/.next/ before this script runs (install.sh handles that).
#
#   ./scripts/install-ui-daemon.sh
#
# Re-running this script is safe — it bootouts the existing agent first,
# rewrites the plist with current paths, and reloads it.
#
# Uninstall:
#   ./scripts/uninstall-ui-daemon.sh

set -euo pipefail

LABEL="com.meridiona.ui"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
TEMPLATE="${SCRIPT_DIR}/${LABEL}.plist"

LAUNCH_AGENTS="${HOME}/Library/LaunchAgents"
PLIST_DEST="${LAUNCH_AGENTS}/${LABEL}.plist"

GUI_TARGET="gui/$(id -u)"

if [[ ! -f "${TEMPLATE}" ]]; then
    echo "✗ template not found: ${TEMPLATE}" >&2
    exit 1
fi

NPM_BIN="$(command -v npm 2>/dev/null || true)"
if [[ -z "${NPM_BIN}" ]]; then
    echo "✗ npm not found in PATH — install Node.js 18+ and re-run" >&2
    exit 1
fi
# Capture the node binary directory so launchd's restricted PATH includes it.
# `npm run start` invokes `next start` whose shebang resolves `node` via PATH;
# NVM / fnm / asdf install node outside /opt/homebrew and /usr/local, so the
# standard launchd PATH misses it. Injecting NODE_BIN_DIR fixes this.
NODE_BIN="$(command -v node 2>/dev/null || true)"
NODE_BIN_DIR_PREFIX=""
if [[ -n "${NODE_BIN}" ]]; then
    NODE_BIN_DIR_PREFIX="$(dirname "${NODE_BIN}"):"
fi

if [[ ! -d "${REPO_ROOT}/ui/.next" ]]; then
    echo "✗ ui/.next not found — run \`cd ui && npm ci && npm run build\` first" >&2
    echo "  (or just run ./install.sh which does it for you)" >&2
    exit 1
fi

mkdir -p "${HOME}/.meridian/logs"
mkdir -p "${LAUNCH_AGENTS}"

echo "→ writing ${PLIST_DEST}"
sed \
    -e "s|{{REPO_ROOT}}|${REPO_ROOT}|g" \
    -e "s|{{HOME}}|${HOME}|g" \
    -e "s|{{NPM_BIN}}|${NPM_BIN}|g" \
    -e "s|{{NODE_BIN_DIR}}|${NODE_BIN_DIR_PREFIX}|g" \
    "${TEMPLATE}" > "${PLIST_DEST}"

if ! plutil -lint "${PLIST_DEST}" >/dev/null; then
    echo "✗ plist failed validation" >&2
    exit 1
fi

echo "→ bootout ${LABEL} (if loaded)"
launchctl bootout "${GUI_TARGET}/${LABEL}" 2>/dev/null || true
# bootout is async — wait until the domain entry actually clears before
# bootstrapping, otherwise launchctl bootstrap can fail with EIO (errno 5).
_bootout_wait=0
while launchctl print "${GUI_TARGET}/${LABEL}" >/dev/null 2>&1; do
    sleep 1
    _bootout_wait=$(( _bootout_wait + 1 ))
    if [[ "${_bootout_wait}" -ge 15 ]]; then
        echo "⚠ ${LABEL} still in launchd domain after 15s — proceeding anyway" >&2
        break
    fi
done

echo "→ bootstrap ${LABEL}"
launchctl enable "${GUI_TARGET}/${LABEL}" 2>/dev/null || true
launchctl bootstrap "${GUI_TARGET}" "${PLIST_DEST}"
launchctl enable "${GUI_TARGET}/${LABEL}"
launchctl kickstart -k "${GUI_TARGET}/${LABEL}"

echo
echo "✓ UI installed and started"
echo
echo "  open  http://localhost:3939                # the dashboard"
echo "  tail -f ~/.meridian/logs/ui.log            # live stdout"
echo "  tail -f ~/.meridian/logs/ui-error.log      # live stderr"
echo "  ${SCRIPT_DIR}/uninstall-ui-daemon.sh       # remove"
