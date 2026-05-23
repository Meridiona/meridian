#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions
# Install OpenObserve as a launchd LaunchAgent under the current user.
# Serves on http://localhost:5080 and auto-starts on login.
#
#   ./scripts/install-openobserve-daemon.sh
#
# Re-running is safe — it bootouts the existing agent first, rewrites the
# plist with current credentials, and reloads it.
#
# Requires MERIDIAN_OO_AUTH=<base64(email:password)> in ~/.meridian/.env.
#
# Uninstall:
#   ./scripts/uninstall-openobserve-daemon.sh

set -euo pipefail

LABEL="com.meridiona.openobserve"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TEMPLATE="${SCRIPT_DIR}/${LABEL}.plist"

LAUNCH_AGENTS="${HOME}/Library/LaunchAgents"
PLIST_DEST="${LAUNCH_AGENTS}/${LABEL}.plist"

GUI_TARGET="gui/$(id -u)"

if [[ ! -f "${TEMPLATE}" ]]; then
    echo "✗ template not found: ${TEMPLATE}" >&2
    exit 1
fi

# Locate the OpenObserve binary.
OO_BIN=""
if [[ -x "${HOME}/.openobserve/openobserve" ]]; then
    OO_BIN="${HOME}/.openobserve/openobserve"
elif command -v openobserve >/dev/null 2>&1; then
    OO_BIN="$(command -v openobserve)"
fi

if [[ -z "${OO_BIN}" ]]; then
    echo "✗ OpenObserve binary not found at ~/.openobserve/openobserve" >&2
    echo "  Run ./install.sh to download it first." >&2
    exit 1
fi

# Read MERIDIAN_OO_AUTH from ~/.meridian/.env and decode it.
ENV_FILE="${HOME}/.meridian/.env"
OO_AUTH=""
if [[ -f "${ENV_FILE}" ]]; then
    OO_AUTH="$(grep -E '^MERIDIAN_OO_AUTH=' "${ENV_FILE}" | cut -d= -f2- | tr -d '[:space:]')" || true
fi

if [[ -z "${OO_AUTH}" ]]; then
    echo "✗ MERIDIAN_OO_AUTH not set in ${ENV_FILE}" >&2
    echo "  Add it:  MERIDIAN_OO_AUTH=\$(printf 'email:password' | base64)" >&2
    exit 1
fi

OO_CREDENTIALS=""
OO_CREDENTIALS="$(printf '%s' "${OO_AUTH}" | base64 --decode 2>/dev/null)" || {
    echo "✗ MERIDIAN_OO_AUTH is not valid base64" >&2
    exit 1
}
OO_EMAIL="${OO_CREDENTIALS%%:*}"
OO_PASSWORD="${OO_CREDENTIALS#*:}"

if [[ -z "${OO_EMAIL}" || -z "${OO_PASSWORD}" || "${OO_EMAIL}" == "${OO_CREDENTIALS}" ]]; then
    echo "✗ MERIDIAN_OO_AUTH decoded to '${OO_CREDENTIALS}' — expected 'email:password'" >&2
    exit 1
fi

mkdir -p "${HOME}/.meridian/logs"
mkdir -p "${HOME}/.openobserve/data"
mkdir -p "${LAUNCH_AGENTS}"

# Write the plist via Python so email/password values with special characters
# are substituted safely without sed delimiter collisions.
echo "→ writing ${PLIST_DEST}"
python3 - "${TEMPLATE}" "${PLIST_DEST}" "${HOME}" "${OO_BIN}" "${OO_EMAIL}" "${OO_PASSWORD}" <<'PYEOF'
import sys
template_path, dest_path, home, oo_bin, oo_email, oo_password = sys.argv[1:]
with open(template_path) as f:
    content = f.read()
for placeholder, value in [
    ("{{HOME}}",         home),
    ("{{OO_BIN}}",       oo_bin),
    ("{{OO_EMAIL}}",     oo_email),
    ("{{OO_PASSWORD}}", oo_password),
]:
    content = content.replace(placeholder, value)
with open(dest_path, "w") as f:
    f.write(content)
PYEOF

if ! plutil -lint "${PLIST_DEST}" >/dev/null; then
    echo "✗ plist failed plutil validation" >&2
    exit 1
fi

if launchctl print "${GUI_TARGET}/${LABEL}" >/dev/null 2>&1; then
    echo "→ bootout existing ${LABEL}"
    launchctl bootout "${GUI_TARGET}" "${PLIST_DEST}" || true
fi

echo "→ bootstrap ${LABEL}"
launchctl bootstrap "${GUI_TARGET}" "${PLIST_DEST}"
launchctl enable "${GUI_TARGET}/${LABEL}"
launchctl kickstart -k "${GUI_TARGET}/${LABEL}"

echo
echo "✓ OpenObserve installed and started"
echo
echo "  open  http://localhost:5080                           # the UI"
echo "  tail -f ~/.meridian/logs/openobserve.log              # live stdout"
echo "  tail -f ~/.meridian/logs/openobserve-error.log        # live stderr"
echo "  ${SCRIPT_DIR}/uninstall-openobserve-daemon.sh         # remove"
