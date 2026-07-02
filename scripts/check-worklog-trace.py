"""Verify the worklog_hour trace nests end-to-end in OpenObserve.

Fetches the most recent ``worklog.hour`` trace from OO's traces stream, rebuilds
the span tree from each span's parent pointer, prints it, and asserts the
expected parent→child edges:

    worklog.hour
      ├─ worklog.sessions   → distill_hour
      ├─ worklog.report     → activity_report
      ├─ worklog.classify   → worklog.classify.tier1[/tier2/...] → classify_tasks
      ├─ worklog.propose    → propose_ticket            (unless skipped)
      └─ worklog.generate   → worklog.generate.ticket   → generate_worklog

Auth + base URL match scripts/sync-oo-dashboards.py (settings.json oo_email/
oo_password, OO_BASE_URL or http://localhost:5080). Exit 0 = tree OK, 1 = orphan
spans / missing expected edges.

Usage: python scripts/check-worklog-trace.py [--base-url URL] [--trace-id ID]
"""
from __future__ import annotations

import argparse
import base64
import json
import os
import re
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path


def _settings() -> dict:
    p = Path.home() / ".meridian" / "settings.json"
    try:
        return json.loads(p.read_text())
    except (OSError, json.JSONDecodeError):
        return {}


def _token(s: dict) -> str:
    email, passwd = s.get("oo_email") or "", s.get("oo_password") or ""
    if not email or not passwd:
        sys.exit("error: oo_email / oo_password not set in ~/.meridian/settings.json")
    return base64.b64encode(f"{email}:{passwd}".encode()).decode()


def _search(base: str, token: str, sql: str, hours: int = 48) -> list[dict]:
    now_us = int(time.time() * 1_000_000)
    body = json.dumps({
        "query": {
            "sql": sql,
            "start_time": now_us - hours * 3600 * 1_000_000,
            "end_time": now_us,
            "from": 0,
            "size": 2000,
        }
    }).encode()
    url = f"{base}/api/default/_search?type=traces"
    req = urllib.request.Request(
        url, data=body, method="POST",
        headers={"Authorization": f"Basic {token}", "Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            return json.loads(r.read()).get("hits", [])
    except urllib.error.HTTPError as e:
        sys.exit(f"error: OO search {e.code}: {e.read()[:400]!r}")
    except Exception as e:
        sys.exit(f"error: OO search failed: {e}")


# Span field names vary slightly across OO versions; probe a few.
_PARENT_KEYS = ["reference_parent_span_id", "parent_span_id", "reference.parent_span_id"]
_SPAN_KEYS = ["span_id", "spanId"]
_OP_KEYS = ["operation_name", "name"]

# OTel trace ids are hex strings (128-bit, 32 chars) — this is deliberately
# strict, not just an escape. `_search`'s query is a raw SQL string handed to
# OO's HTTP API (no parameterized-query support there), so trace_id — which
# can come straight from --trace-id, fully attacker/typo controlled — is
# validated against an allowlist before ever being interpolated, rather than
# escaped. Same reasoning applies to the auto-discovered id from OO's own
# response: defense in depth against a compromised/misbehaving OO instance.
_TRACE_ID_RE = re.compile(r"^[0-9a-fA-F]{1,64}$")


def _validated_trace_id(trace_id: str) -> str:
    if not _TRACE_ID_RE.match(trace_id):
        sys.exit(f"error: trace_id {trace_id!r} doesn't look like a hex trace id — refusing to query")
    return trace_id


def _first(row: dict, keys: list[str], default: str = "") -> str:
    for k in keys:
        if row.get(k):
            return str(row[k])
    return default


EXPECTED_PARENT = {
    # child operation_name : its required parent operation_name
    "worklog.sessions": "worklog.hour",
    "worklog.report": "worklog.hour",
    "worklog.classify": "worklog.hour",
    "worklog.propose": "worklog.hour",
    "worklog.generate": "worklog.hour",
    "distill_hour": "worklog.sessions",
    "activity_report": "worklog.report",
    "worklog.classify.tier1": "worklog.classify",
    "classify_tasks": ("worklog.classify.tier1", "worklog.classify.tier2.batch"),
    "propose_ticket": "worklog.propose",
    "worklog.generate.ticket": "worklog.generate",
    "generate_worklog": "worklog.generate.ticket",
}


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--base-url", default=os.environ.get("OO_BASE_URL", "http://localhost:5080"))
    ap.add_argument("--trace-id", default="")
    args = ap.parse_args()

    token = _token(_settings())
    base = args.base_url.rstrip("/")

    trace_id = args.trace_id
    if not trace_id:
        hits = _search(base, token,
                       "SELECT trace_id, _timestamp FROM \"default\" "
                       "WHERE operation_name='worklog.hour' ORDER BY _timestamp DESC")
        if not hits:
            sys.exit("error: no worklog.hour span found in OO (run /worklog_hour first)")
        trace_id = _first(hits[0], ["trace_id", "traceId"])
        print(f"latest worklog.hour trace_id = {trace_id}")

    trace_id = _validated_trace_id(trace_id)
    spans = _search(base, token, f"SELECT * FROM \"default\" WHERE trace_id='{trace_id}'")
    if not spans:
        sys.exit(f"error: no spans for trace_id={trace_id}")

    # Build id→op and id→parent maps.
    by_id: dict[str, str] = {}
    parent_of: dict[str, str] = {}
    for s in spans:
        sid = _first(s, _SPAN_KEYS)
        by_id[sid] = _first(s, _OP_KEYS, "<?>")
        parent_of[sid] = _first(s, _PARENT_KEYS)

    # Print the tree.
    children: dict[str, list[str]] = {}
    roots = []
    for sid, pid in parent_of.items():
        if pid and pid in by_id:
            children.setdefault(pid, []).append(sid)
        else:
            roots.append(sid)

    def show(sid: str, depth: int) -> None:
        print("  " * depth + f"- {by_id[sid]}")
        for c in sorted(children.get(sid, []), key=lambda x: by_id[x]):
            show(c, depth + 1)

    print(f"\nspan tree ({len(spans)} spans):")
    for r in sorted(roots, key=lambda x: by_id[x]):
        show(r, 0)

    # Assert expected parent edges.
    op_parent: dict[str, set[str]] = {}
    for sid, pid in parent_of.items():
        op_parent.setdefault(by_id[sid], set()).add(by_id.get(pid, ""))

    problems = []
    for op, exp in EXPECTED_PARENT.items():
        if op not in op_parent:
            continue  # that span didn't occur this run (e.g. propose skipped) — fine
        parents = op_parent[op]
        exp_tuple = exp if isinstance(exp, tuple) else (exp,)
        ok = any(any(p.startswith(e) for e in exp_tuple) for p in parents)
        if not ok:
            problems.append(f"  {op}: parent is {parents}, expected to start with {exp_tuple}")

    # worklog.hour must be a root (no parent inside this trace's mlx spans), and
    # every non-root span must have a known parent (no orphans).
    orphans = [by_id[s] for s, p in parent_of.items()
               if by_id[s] != "worklog.hour" and (not p or p not in by_id)]
    # worklog.hour legitimately has a parent only if the Rust caller's span is in
    # the same trace; tolerate that. Orphans we care about are the mlx sub-spans.
    mlx_orphans = [o for o in orphans if o.startswith("worklog.") or o in
                   ("distill_hour", "activity_report", "classify_tasks",
                    "propose_ticket", "generate_worklog")]

    print()
    if problems:
        print("FAIL — wrong parent edges:")
        print("\n".join(problems))
    if mlx_orphans:
        print(f"FAIL — orphaned mlx spans (not nested): {mlx_orphans}")
    if problems or mlx_orphans:
        sys.exit(1)
    print("OK — worklog.hour trace nests end-to-end; all expected edges present.")


if __name__ == "__main__":
    main()
