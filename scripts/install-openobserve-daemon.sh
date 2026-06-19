#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
# Install OpenObserve as a launchd LaunchAgent under the current user.
# Serves on http://localhost:5080 and auto-starts on login.
#
#   ./scripts/install-openobserve-daemon.sh
#
# Re-running is safe — it bootouts the existing agent first, rewrites the
# plist with current credentials, and reloads it.
#
# Credentials come from settings.json (oo_email/oo_password, set in the
# dashboard Settings). MERIDIAN_OO_AUTH in <repo>/.env is DEPRECATED and used
# only as a fallback; with no credentials anywhere the agent is installed
# stopped and the dashboard toggle starts it once credentials are set.
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
    echo "→ OpenObserve binary not found — downloading v0.90.3..."
    _oo_arch="$(uname -m)"
    case "$_oo_arch" in
        arm64)  _oo_arch="arm64" ;;
        x86_64) _oo_arch="amd64" ;;
        *) echo "✗ Unsupported arch: $_oo_arch" >&2; exit 1 ;;
    esac
    # GitHub release assets were removed for recent versions; binaries now live on
    # the official downloads host. Trace deep-linking (dashboard drilldown into a
    # single trace's spans) needs a modern build, so we pin a current stable.
    # KEEP IN SYNC: the same version is pinned in install.sh — bump both together.
    _oo_ver="v0.90.3"
    _oo_url="https://downloads.openobserve.ai/releases/openobserve/${_oo_ver}/openobserve-${_oo_ver}-darwin-${_oo_arch}.tar.gz"
    mkdir -p "${HOME}/.openobserve"
    if curl -fsSL -o "${HOME}/.openobserve/openobserve.tar.gz" "$_oo_url" \
        && tar -xzf "${HOME}/.openobserve/openobserve.tar.gz" -C "${HOME}/.openobserve" \
        && [[ -x "${HOME}/.openobserve/openobserve" ]]; then
        rm -f "${HOME}/.openobserve/openobserve.tar.gz"
        OO_BIN="${HOME}/.openobserve/openobserve"
        echo "✓ OpenObserve ${_oo_ver} downloaded"
    else
        rm -f "${HOME}/.openobserve/openobserve.tar.gz"
        echo "✗ Download failed from ${_oo_url}" >&2
        echo "  Install manually: https://openobserve.ai/docs/install/" >&2
        exit 1
    fi
fi

# Resolve OpenObserve root credentials. settings.json (written by the dashboard
# Settings page) is the canonical source; MERIDIAN_OO_AUTH in <repo>/.env is
# DEPRECATED and honoured only as a fallback for not-yet-migrated installs.
# With no credentials anywhere, the plist is written with placeholders and the
# service is left stopped — enabling OpenObserve Export in the dashboard
# patches real credentials into the plist (POST /api/openobserve) before the
# service's first start, which is when OpenObserve creates its root account.
_get_setting() {
    python3 -c "import json,sys;print(json.load(open(sys.argv[1])).get(sys.argv[2]) or '')" "$1" "$2" 2>/dev/null || true
}

OO_EMAIL=""
OO_PASSWORD=""
for _settings in "${HOME}/.meridian/settings.json" "$(cd "${SCRIPT_DIR}/.." && pwd)/settings.json"; do
    [[ -f "${_settings}" ]] || continue
    OO_EMAIL="$(_get_setting "${_settings}" oo_email)"
    OO_PASSWORD="$(_get_setting "${_settings}" oo_password)"
    if [[ -n "${OO_EMAIL}" && -n "${OO_PASSWORD}" ]]; then
        echo "→ using OpenObserve credentials from ${_settings}"
        break
    fi
    OO_EMAIL=""; OO_PASSWORD=""
done

if [[ -z "${OO_EMAIL}" ]]; then
    ENV_FILE="$(cd "${SCRIPT_DIR}/.." && pwd)/.env"
    OO_AUTH=""
    if [[ -f "${ENV_FILE}" ]]; then
        OO_AUTH="$(grep -E '^MERIDIAN_OO_AUTH=' "${ENV_FILE}" | cut -d= -f2- | tr -d '[:space:]')" || true
    fi
    if [[ -n "${OO_AUTH}" ]]; then
        echo "  ⚠ MERIDIAN_OO_AUTH is DEPRECATED — set OpenObserve credentials in the dashboard Settings instead" >&2
        OO_CREDENTIALS="$(printf '%s' "${OO_AUTH}" | base64 --decode 2>/dev/null)" || OO_CREDENTIALS=""
        OO_EMAIL="${OO_CREDENTIALS%%:*}"
        OO_PASSWORD="${OO_CREDENTIALS#*:}"
        if [[ -z "${OO_EMAIL}" || -z "${OO_PASSWORD}" || "${OO_EMAIL}" == "${OO_CREDENTIALS}" ]]; then
            OO_EMAIL=""; OO_PASSWORD=""
        fi
    fi
fi

_no_creds=0
if [[ -z "${OO_EMAIL}" || -z "${OO_PASSWORD}" ]]; then
    _no_creds=1
    OO_EMAIL="setup-pending@meridian.local"
    OO_PASSWORD="setup-pending"
    echo "→ no OpenObserve credentials yet — installing the agent stopped; enable"
    echo "  OpenObserve Export in the dashboard Settings to set them and start it"
fi

mkdir -p "${HOME}/.meridian/logs"
mkdir -p "${HOME}/.openobserve/data"
mkdir -p "${LAUNCH_AGENTS}"

# Remove legacy ai.openobserve agent — it conflicts with com.meridiona.openobserve
# (both try to bind port 5080, causing a crash-loop on the Meridian plist).
_legacy_plist="${LAUNCH_AGENTS}/ai.openobserve.plist"
if [[ -f "${_legacy_plist}" ]]; then
    echo "→ removing legacy ai.openobserve agent"
    launchctl bootout "gui/$(id -u)/ai.openobserve" 2>/dev/null || true
    rm -f "${_legacy_plist}"
    echo "✓ legacy agent removed"
fi

# Write the launcher script. run.sh reads ~/.openobserve/.log_level for
# RUST_LOG (default: warn) and sets memory caps before exec-ing the binary.
echo "→ writing ${HOME}/.openobserve/run.sh"
cat > "${HOME}/.openobserve/run.sh" <<'RUNEOF'
#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
# OpenObserve launcher. Called by launchd (com.meridiona.openobserve).
#
# Log level override (dev):
#   echo info   > ~/.openobserve/.log_level   # verbose
#   echo warn   > ~/.openobserve/.log_level   # default (quiet)
#   echo debug  > ~/.openobserve/.log_level   # very verbose
#   rm ~/.openobserve/.log_level              # back to default
#   launchctl kickstart -k gui/$(id -u)/com.meridiona.openobserve

set -euo pipefail

_level_file="${HOME}/.openobserve/.log_level"
if [[ -f "${_level_file}" ]]; then
    RUST_LOG="$(tr -d '[:space:]' < "${_level_file}")"
else
    RUST_LOG="warn"
fi
export RUST_LOG

export ZO_MEMORY_CACHE_MAX_SIZE=512          # 512 MB (unit is MB in v0.15+; bytes caused panic)
export ZO_DATAFUSION_POOL_SIZE=4096          # 4 GB  (unit is MB in v0.15+)

exec "${HOME}/.openobserve/openobserve"
RUNEOF
chmod +x "${HOME}/.openobserve/run.sh"
echo "✓ run.sh written"

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

echo "→ bootout ${LABEL} (if loaded)"
launchctl bootout "${GUI_TARGET}/${LABEL}" 2>/dev/null || true
# Wait until launchd confirms the service is gone before re-bootstrapping.
# A fixed sleep is unreliable — on slower machines or when the prior process
# takes time to exit, bootstrap can fail with EIO (errno 5) if the domain
# entry hasn't been fully removed yet.
_bootout_wait=0
while launchctl print "${GUI_TARGET}/${LABEL}" >/dev/null 2>&1; do
    sleep 1
    _bootout_wait=$(( _bootout_wait + 1 ))
    if [[ "${_bootout_wait}" -ge 15 ]]; then
        echo "⚠ ${LABEL} still in launchd domain after 15s — proceeding anyway" >&2
        break
    fi
done

# Respect the runtime toggle: the service runs only when "OpenObserve Export"
# is enabled in Settings. The plist is always installed so the UI's toggle
# (POST /api/openobserve) can start/stop the service on demand; here we only
# decide the INITIAL state. No settings.json anywhere → off (matches
# RuntimeSettings::default and the UI default).
_otlp_enabled() {
    local f
    for f in "${HOME}/.meridian/settings.json" "$(cd "${SCRIPT_DIR}/.." && pwd)/settings.json"; do
        [[ -f "$f" ]] || continue
        grep -q '"otlp_enabled"[[:space:]]*:[[:space:]]*true' "$f" && return 0
        return 1
    done
    return 1
}

# Provision the bundled dashboards (services/observability/dashboards/*.json)
# into OpenObserve via its REST API. Idempotent: a dashboard is created only if
# no dashboard with the same title already exists (create endpoint always mints
# a fresh dashboardId, so a blind POST on every install would duplicate). Runs
# only once the service is up and reachable; degrades silently (never fails the
# install) when OpenObserve is unreachable or export is off.
_import_dashboards() {
    local dash_dir
    dash_dir="$(cd "${SCRIPT_DIR}/.." && pwd)/services/observability/dashboards"
    [[ -d "${dash_dir}" ]] || return 0
    OO_EMAIL="${OO_EMAIL}" OO_PASSWORD="${OO_PASSWORD}" python3 - "${dash_dir}" <<'PYEOF'
import base64, glob, json, os, sys, time, urllib.request, urllib.error

dash_dir = sys.argv[1]
base = "http://localhost:5080"
org = "default"
auth = base64.b64encode(f'{os.environ["OO_EMAIL"]}:{os.environ["OO_PASSWORD"]}'.encode()).decode()

def call(method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(
        base + path, data=data, method=method,
        headers={"Authorization": "Basic " + auth, "Content-Type": "application/json"},
    )
    try:
        r = urllib.request.urlopen(req, timeout=15)
        return r.status, r.read()
    except urllib.error.HTTPError as e:
        return e.code, e.read()
    except Exception as e:  # connection refused, DNS, timeout, …
        return None, str(e).encode()

# Wait for the service to accept authenticated requests (just-kickstarted).
for _ in range(30):
    st, _b = call("GET", "/config")
    if st == 200:
        break
    time.sleep(1)
else:
    print("  ⚠ OpenObserve not reachable — skipping dashboard import (import later by re-running this script)")
    sys.exit(0)

st, body = call("GET", f"/api/{org}/dashboards")
if st != 200:
    print(f"  ⚠ could not list dashboards ({st}) — skipping dashboard import")
    sys.exit(0)
existing = set()
for d in json.loads(body).get("dashboards", []):
    for v in ("v1", "v2", "v3", "v4", "v5", "v6"):
        if d.get(v) and d[v].get("title"):
            existing.add(d[v]["title"])

created = skipped = failed = 0
for path in sorted(glob.glob(os.path.join(dash_dir, "*.json"))):
    try:
        dash = json.load(open(path))
    except Exception as e:
        print(f"  ⚠ {os.path.basename(path)}: invalid JSON ({e}) — skipped")
        failed += 1
        continue
    title = dash.get("title", "")
    if title in existing:
        skipped += 1
        continue
    st, resp = call("POST", f"/api/{org}/dashboards?folder=default", dash)
    if st in (200, 201):
        created += 1
        print(f"  ✓ imported dashboard: {title}")
    else:
        failed += 1
        print(f"  ⚠ failed to import {os.path.basename(path)} ({st}): {resp[:200].decode(errors='replace')}")

print(f"→ dashboards: {created} imported, {skipped} already present, {failed} failed")
PYEOF
    return 0
}

if [[ "${_no_creds}" -eq 0 ]] && _otlp_enabled; then
    echo "→ bootstrap ${LABEL} (OpenObserve Export is enabled in settings)"
    launchctl bootstrap "${GUI_TARGET}" "${PLIST_DEST}"
    launchctl enable "${GUI_TARGET}/${LABEL}"
    launchctl kickstart -k "${GUI_TARGET}/${LABEL}"
    echo
    echo "✓ OpenObserve installed and started"
    echo "→ importing bundled dashboards"
    _import_dashboards || true
else
    launchctl disable "${GUI_TARGET}/${LABEL}" 2>/dev/null || true
    echo
    echo "✓ OpenObserve installed (service left stopped — OpenObserve Export is"
    echo "  disabled in Settings; enable the toggle in the dashboard to start it)"
fi
echo
echo "  open  http://localhost:5080                           # the UI"
echo "  tail -f ~/.meridian/logs/openobserve.log              # live stdout"
echo "  tail -f ~/.meridian/logs/openobserve-error.log        # live stderr"
echo "  ${SCRIPT_DIR}/uninstall-openobserve-daemon.sh         # remove"
