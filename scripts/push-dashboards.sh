#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
# Push (upsert) all dashboards from services/observability/dashboards/*.json into
# the local OpenObserve instance. Safe to run any time — existing dashboards are
# updated in-place, new ones are created.
#
#   ./scripts/push-dashboards.sh
#
# Credentials are read from settings.json (oo_email/oo_password), same as
# install-openobserve-daemon.sh.

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

_get_setting() {
    python3 -c "import json,sys;print(json.load(open(sys.argv[1])).get(sys.argv[2]) or '')" "$1" "$2" 2>/dev/null || true
}

OO_EMAIL=""
OO_PASSWORD=""
for _settings in "${HOME}/.meridian/settings.json" "${REPO_ROOT}/settings.json"; do
    [[ -f "${_settings}" ]] || continue
    OO_EMAIL="$(_get_setting "${_settings}" oo_email)"
    OO_PASSWORD="$(_get_setting "${_settings}" oo_password)"
    [[ -n "${OO_EMAIL}" && -n "${OO_PASSWORD}" ]] && break
    OO_EMAIL=""; OO_PASSWORD=""
done

if [[ -z "${OO_EMAIL}" || -z "${OO_PASSWORD}" ]]; then
    echo "⚠ OpenObserve credentials not found in settings.json — skipping dashboard push" >&2
    exit 0
fi

OO_EMAIL="${OO_EMAIL}" OO_PASSWORD="${OO_PASSWORD}" python3 - "${REPO_ROOT}/services/observability/dashboards" <<'PYEOF'
import base64, glob, json, os, sys, urllib.request, urllib.error

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
        r = urllib.request.urlopen(req, timeout=10)
        return r.status, r.read()
    except urllib.error.HTTPError as e:
        return e.code, e.read()
    except Exception as e:
        return None, str(e).encode()

st, _ = call("GET", "/config")
if st != 200:
    print("⚠ OpenObserve not reachable on localhost:5080 — skipping")
    sys.exit(0)

st, body = call("GET", f"/api/{org}/dashboards")
if st != 200:
    print(f"⚠ could not list dashboards ({st}) — skipping")
    sys.exit(0)

# title -> dashboard_id (for existing dashboards)
id_map = {}
for d in json.loads(body).get("dashboards", []):
    did = d.get("dashboard_id", "")
    for v in ("v1","v2","v3","v4","v5","v6","v7","v8"):
        if d.get(v) and d[v].get("title"):
            id_map[d[v]["title"]] = did
            break

created = updated = failed = 0
for path in sorted(glob.glob(os.path.join(dash_dir, "*.json"))):
    try:
        dash = json.load(open(path))
    except Exception as e:
        print(f"  ⚠ {os.path.basename(path)}: invalid JSON ({e}) — skipped")
        failed += 1
        continue
    title = dash.get("title", "")
    if title in id_map:
        # OO PUT requires a hash that changes on every write and can't be round-tripped
        # reliably. Delete + re-create is simpler and always works.
        did = id_map[title]
        st_d, _ = call("DELETE", f"/api/{org}/dashboards/{did}")
        if st_d not in (200, 204):
            failed += 1
            print(f"  ⚠ delete failed {title} ({st_d}) — skipping")
            continue
        st, resp = call("POST", f"/api/{org}/dashboards?folder=default", dash)
        if st in (200, 201):
            updated += 1
            print(f"  ✓ updated: {title}")
        else:
            failed += 1
            print(f"  ⚠ re-create failed {title} ({st}): {resp[:120].decode(errors='replace')}")
    else:
        st, resp = call("POST", f"/api/{org}/dashboards?folder=default", dash)
        if st in (200, 201):
            created += 1
            print(f"  ✓ created: {title}")
        else:
            failed += 1
            print(f"  ⚠ create failed {title} ({st}): {resp[:120].decode(errors='replace')}")

print(f"→ dashboards: {created} created, {updated} updated, {failed} failed")
PYEOF
