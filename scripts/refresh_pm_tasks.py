#!/usr/bin/env python3
# meridian — normalises screenpipe activity into structured app sessions
"""One-shot refresh of pm_tasks from Jira via REST (stdlib only).

The Rust daemon only refreshes pm_tasks once at startup (see
src/intelligence/providers/jira.rs::refresh_if_stale). When the cache goes
stale during a long daemon run, this script repopulates it without restarting
the daemon. Mirrors the Rust upsert exactly (30-min expiry).

By default also prunes any provider='jira' rows whose task_key is no longer
returned by the JQL — keeps the cache in sync when tickets are closed,
reassigned, or deleted. Cascades into pm_task_embeddings; ticket_links is
left alone (it's audit history). Skipped automatically when len(issues) ==
--limit to avoid wiping the tail on a truncated response; override with
--no-prune.

Usage:
    python3 scripts/refresh_pm_tasks.py
    python3 scripts/refresh_pm_tasks.py --jql "project=KAN ORDER BY updated DESC"
    python3 scripts/refresh_pm_tasks.py --db /path/to/meridian.db
    python3 scripts/refresh_pm_tasks.py --no-prune

Reads credentials from JIRA_URL / JIRA_EMAIL / JIRA_API_TOKEN, loaded from the
repo-root .env (does not override existing env vars). Nothing is read from
outside the repo.
"""
from __future__ import annotations

import argparse
import base64
import json
import logging
import os
import sqlite3
import sys
import urllib.error
import urllib.request
from pathlib import Path

DEFAULT_JQL = (
    "assignee = currentUser() AND statusCategory != Done AND type IN (Task, Feature) ORDER BY updated DESC"
)
DEFAULT_DB = Path.home() / ".meridian" / "meridian.db"

# ── env loading ────────────────────────────────────────────────────────────────
def _load_env(path: Path) -> int:
    if not path.exists():
        return 0
    n = 0
    for line in path.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        k, _, v = line.partition("=")
        k, v = k.strip(), v.strip().strip('"').strip("'")
        if k and k not in os.environ:
            os.environ[k] = v
            n += 1
    return n


# ── Jira REST ──────────────────────────────────────────────────────────────────
def jira_search(base_url: str, email: str, token: str, jql: str, limit: int) -> list[dict]:
    creds = base64.b64encode(f"{email}:{token}".encode()).decode()
    url = f"{base_url.rstrip('/')}/rest/api/3/search/jql"
    payload = json.dumps({
        "jql": jql,
        "fields": ["summary", "description", "issuetype", "project", "updated", "parent", "status"],
        "maxResults": limit,
    }).encode()
    req = urllib.request.Request(
        url,
        data=payload,
        method="POST",
        headers={
            "Authorization": f"Basic {creds}",
            "Content-Type": "application/json",
            "Accept": "application/json",
        },
    )
    with urllib.request.urlopen(req, timeout=60) as resp:
        body = json.loads(resp.read().decode())
    return body.get("issues", [])


def adf_to_text(node) -> str:
    if not node:
        return ""
    if isinstance(node, str):
        return node
    if isinstance(node, dict):
        if node.get("type") == "text":
            return node.get("text", "")
        return "".join(adf_to_text(c) for c in node.get("content", []))
    if isinstance(node, list):
        return "".join(adf_to_text(c) for c in node)
    return ""


# ── DB upsert (mirrors src/intelligence/providers/jira.rs upsert) ─────────────
UPSERT_SQL = """
INSERT INTO pm_tasks (
    task_key, provider, title, description_text,
    status_category, issue_type, project_key, url,
    updated_at, fetched_at,
    parent_key, epic_title
) VALUES (?, 'jira', ?, ?, ?, ?, ?, ?, ?,
          strftime('%Y-%m-%dT%H:%M:%SZ', 'now'),
          ?, ?)
ON CONFLICT(task_key) DO UPDATE SET
    title            = excluded.title,
    description_text = excluded.description_text,
    status_category  = excluded.status_category,
    issue_type       = excluded.issue_type,
    project_key      = excluded.project_key,
    url              = excluded.url,
    updated_at       = excluded.updated_at,
    fetched_at       = excluded.fetched_at,
    parent_key       = excluded.parent_key,
    epic_title       = excluded.epic_title
"""

SYNC_STATE_SQL = """
INSERT INTO pm_sync_state (provider, last_synced_at)
VALUES ('jira', strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
ON CONFLICT(provider) DO UPDATE SET last_synced_at = excluded.last_synced_at
"""

STATUS_CATEGORY_MAP = {"done": "done", "indeterminate": "in_progress"}


def normalise(issue: dict, base_url: str) -> tuple:
    fields = issue.get("fields") or {}
    status = fields.get("status") or {}
    cat_key = ((status.get("statusCategory") or {}).get("key")) or "new"
    # Map Jira status categories to normalized form: todo, in_progress, done
    status_cat = STATUS_CATEGORY_MAP.get(cat_key, "todo")
    parent = fields.get("parent")
    parent_key = parent.get("key") if parent else None
    parent_title = None
    if parent and parent.get("fields"):
        parent_title = parent.get("fields", {}).get("summary")
    return (
        issue.get("key", ""),
        fields.get("summary") or "",
        adf_to_text(fields.get("description")),
        status_cat,
        (fields.get("issuetype") or {}).get("name") or "",
        (fields.get("project") or {}).get("key") or "",
        f"{base_url.rstrip('/')}/browse/{issue.get('key', '')}",
        fields.get("updated") or "",
        parent_key,
        parent_title,
    )


# ── main ──────────────────────────────────────────────────────────────────────
def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--jql", default=DEFAULT_JQL)
    ap.add_argument("--limit", type=int, default=50)
    ap.add_argument("--db", type=Path, default=Path(os.environ.get("MERIDIAN_DB", DEFAULT_DB)))
    ap.add_argument(
        "--no-prune",
        action="store_true",
        help="skip deleting jira rows that are no longer returned by the JQL",
    )
    args = ap.parse_args()

    logging.basicConfig(level=logging.INFO, format="%(asctime)s [%(levelname)s] %(message)s")
    log = logging.getLogger("refresh_pm_tasks")

    repo_root = Path(__file__).resolve().parents[1]
    # Single source of truth: the repo-root .env (nothing read outside the repo).
    n = _load_env(repo_root / ".env")
    if n:
        log.info("loaded %d vars from %s", n, repo_root / ".env")

    base_url = os.environ.get("JIRA_BASE_URL", os.environ.get("JIRA_URL", "")).rstrip("/")
    email = os.environ.get("JIRA_EMAIL", "")
    token = os.environ.get("JIRA_API_TOKEN", "")
    missing = [k for k, v in (("JIRA_BASE_URL or JIRA_URL", base_url), ("JIRA_EMAIL", email), ("JIRA_API_TOKEN", token)) if not v]
    if missing:
        log.error("missing env: %s", ", ".join(missing))
        return 2

    log.info("jira: %s  jql=%r  limit=%d", base_url, args.jql, args.limit)
    log.info("meridian.db: %s", args.db)

    try:
        issues = jira_search(base_url, email, token, args.jql, args.limit)
    except urllib.error.HTTPError as e:
        body = e.read().decode(errors="replace")[:500]
        log.error("jira HTTP %d: %s", e.code, body)
        return 3
    except urllib.error.URLError as e:
        log.error("jira request failed: %s", e)
        return 3

    log.info("fetched %d issues", len(issues))
    if not issues:
        log.warning("nothing to upsert")
        return 0

    fetched_keys = {issue.get("key", "") for issue in issues if issue.get("key")}

    conn = sqlite3.connect(args.db)
    try:
        for issue in issues:
            norm = normalise(issue, base_url)
            log.info("upserting %s", issue.get("key"))
            conn.execute(UPSERT_SQL, norm)

        pruned = 0
        if args.no_prune:
            log.info("prune: skipped (--no-prune)")
        elif len(issues) >= args.limit:
            log.warning(
                "prune: skipped — fetched %d == --limit; result may be truncated. "
                "Re-run with a higher --limit to enable pruning.",
                len(issues),
            )
        elif fetched_keys:
            placeholders = ",".join("?" * len(fetched_keys))
            keys = list(fetched_keys)
            # Cascade FK target first (pm_task_embeddings.task_key REFERENCES pm_tasks).
            conn.execute(
                f"DELETE FROM pm_task_embeddings "
                f"WHERE task_key IN ("
                f"  SELECT task_key FROM pm_tasks "
                f"  WHERE provider = 'jira' AND task_key NOT IN ({placeholders})"
                f")",
                keys,
            )
            cur = conn.execute(
                f"DELETE FROM pm_tasks "
                f"WHERE provider = 'jira' AND task_key NOT IN ({placeholders})",
                keys,
            )
            pruned = cur.rowcount or 0

        conn.execute(SYNC_STATE_SQL)
        conn.commit()
    finally:
        conn.close()

    log.info("upserted %d rows; pruned %d stale", len(issues), pruned)
    return 0


if __name__ == "__main__":
    sys.exit(main())
