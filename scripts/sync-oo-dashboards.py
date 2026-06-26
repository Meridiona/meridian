"""Push every dashboard JSON in services/observability/dashboards/ to the local
OpenObserve instance.

Credentials are read from ~/.meridian/settings.json (oo_email / oo_password),
matching how the Rust daemon resolves them. The OO base URL defaults to
http://localhost:5080 but can be overridden with OO_BASE_URL.

Strategy per file:
  1. GET /api/default/dashboards — build a title→id map of existing dashboards.
  2. If the file's title is already in OO → DELETE the old one, then POST the new body.
  3. If it's new → POST directly.

This is the only safe upsert path: OO's PUT /api/default/dashboards/{id}
requires a hash field that is never populated in the GET response (OO bug),
so DELETE+POST is used instead.

Usage:
    python scripts/sync-oo-dashboards.py [--dry-run] [--base-url http://…]
"""

import argparse
import base64
import json
import os
import sys
import urllib.error
import urllib.request
from pathlib import Path


def _settings() -> dict:
    p = Path.home() / ".meridian" / "settings.json"
    try:
        return json.loads(p.read_text())
    except (OSError, json.JSONDecodeError):
        return {}


def _token(settings: dict) -> str:
    email = settings.get("oo_email") or ""
    passwd = settings.get("oo_password") or ""
    if not email or not passwd:
        sys.exit(
            "error: oo_email / oo_password not set in ~/.meridian/settings.json — "
            "configure them via the Meridian dashboard Settings page"
        )
    return base64.b64encode(f"{email}:{passwd}".encode()).decode()


def _req(method: str, url: str, token: str, body: bytes | None = None):
    headers = {"Authorization": f"Basic {token}"}
    if body is not None:
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(url, data=body, method=method, headers=headers)
    try:
        resp = urllib.request.urlopen(req, timeout=10)
        return resp.status, json.loads(resp.read())
    except urllib.error.HTTPError as e:
        return e.code, json.loads(e.read() or b"{}")


def _list_existing(base: str, token: str) -> dict[str, str]:
    """Return {title: dashboardId} for every dashboard in OO."""
    status, data = _req("GET", f"{base}/api/default/dashboards", token)
    if status != 200:
        sys.exit(f"error: GET /api/default/dashboards returned {status}: {data}")
    result = {}
    for entry in data.get("dashboards", []):
        inner = next((entry[k] for k in ["v6", "v5", "v4", "v3", "v2", "v1"] if entry.get(k)), None)
        if inner:
            result[inner["title"]] = inner["dashboardId"]
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dry-run", action="store_true", help="Print what would happen, don't change OO")
    parser.add_argument("--base-url", default=os.environ.get("OO_BASE_URL", "http://localhost:5080"))
    args = parser.parse_args()

    repo_root = Path(__file__).parent.parent
    dash_dir = repo_root / "services" / "observability" / "dashboards"
    files = sorted(dash_dir.glob("*.json"))
    if not files:
        print(f"no dashboard JSON files found in {dash_dir}")
        return

    settings = _settings()
    token = _token(settings)
    base = args.base_url.rstrip("/")

    existing = _list_existing(base, token)
    print(f"OpenObserve @ {base} — {len(existing)} existing dashboards, {len(files)} local files\n")

    ok = err = 0
    for path in files:
        body = json.loads(path.read_text())
        title = body.get("title", path.stem)
        action = "update" if title in existing else "create"

        if args.dry_run:
            dash_id = existing.get(title, "(new)")
            print(f"  [dry-run] {action}: {title!r}  ({path.name}  id={dash_id})")
            continue

        # POST the new version first; only DELETE the old one on success to
        # avoid leaving the dashboard absent if the POST fails.
        status, result = _req("POST", f"{base}/api/default/dashboards", token, json.dumps(body).encode())
        if status == 200:
            inner = next((result[k] for k in ["v6", "v5", "v4", "v3", "v2", "v1"] if result.get(k)), {})
            new_id = inner.get("dashboardId", "?")
            if action == "update":
                dash_id = existing[title]
                del_status, _ = _req("DELETE", f"{base}/api/default/dashboards/{dash_id}", token)
                if del_status != 200:
                    print(f"  ⚠ {title!r}: POST ok (id={new_id}) but DELETE {dash_id} failed ({del_status}); old copy left in place")
            print(f"  ✓ {action}: {title!r}  → id={new_id}  ({path.name})")
            ok += 1
        else:
            msg = result.get("message", result) if isinstance(result, dict) else result
            print(f"  ✗ {action}: {title!r} failed ({status}): {msg}")
            err += 1

    if not args.dry_run:
        print(f"\n{ok} synced, {err} failed")
        if err:
            sys.exit(1)


if __name__ == "__main__":
    main()
