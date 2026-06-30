#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
# Run distill → activity_report → tier-1 classify → tier-2 classify (if tier-1 misses).
#
# Usage:
#   bash scripts/run-classify-tier1.sh [HOUR]
#
# HOUR: local hour label in YYYY-MM-DDTHH format. Defaults to current local hour.
# Each classify call is traced server-side to OpenObserve (the server owns the spans).
#
# Example:
#   bash scripts/run-classify-tier1.sh 2026-06-29T10

set -euo pipefail

SERVER="http://127.0.0.1:7823"
DB_PATH="${MERIDIAN_DB:-$HOME/.meridian/meridian.db}"
HOUR="${1:-$(date '+%Y-%m-%dT%H')}"

echo "→ hour: $HOUR"
echo "→ db:   $DB_PATH"
echo ""

if ! curl -sf "$SERVER/health" >/dev/null 2>&1; then
    echo "✗ MLX server not running at $SERVER — start it with dev-start.sh" >&2
    exit 1
fi

# --- 1. Distill ---
echo "→ distilling sessions …"
DISTILL_OUT=$(curl -sf -X POST "$SERVER/distill_hour" \
    -H "Content-Type: application/json" \
    -d "{\"hour\":\"$HOUR\",\"db_path\":\"$DB_PATH\"}")

NSESS=$(python3 -c "import json,sys; print(json.loads(sys.argv[1])['nsess'])" "$DISTILL_OUT")
echo "   sessions: $NSESS"

if [ "$NSESS" -eq 0 ]; then
    echo "✗ no sessions for $HOUR — nothing to classify"
    exit 0
fi

# --- 2. Activity report ---
echo "→ running activity_report …"
REPORT=$(python3 - "$DISTILL_OUT" "$DB_PATH" <<'PYEOF'
import json, sys, urllib.request

distill = json.loads(sys.argv[1])
db_path = sys.argv[2]
payload = json.dumps({"body": distill["body"], "label": distill["label"], "db_path": db_path}).encode()
req = urllib.request.Request(
    "http://127.0.0.1:7823/activity_report",
    data=payload, headers={"Content-Type": "application/json"}, method="POST",
)
with urllib.request.urlopen(req, timeout=600) as resp:
    result = json.load(resp)
print(result["report"])
PYEOF
)
echo "   report: ${#REPORT} chars"

# --- 3. Classify (tier-1 → tier-2 fallthrough) ---
echo "→ running task classification …"
python3 - "$DB_PATH" "$HOUR" "$REPORT" "$SERVER" <<'PYEOF'
import json, sys
sys.path.insert(0, "services")

from agents.worklog_pipeline import db as wdb
from agents.worklog_pipeline.classifier import (
    BATCH, Candidate, classify_tier1, classify_tier2_batch,
)
from agents.worklog_pipeline.pipeline import HourContext

db_path  = sys.argv[1]
hour     = sys.argv[2]
report   = sys.argv[3]
server   = sys.argv[4]

conn = wdb.open_db(db_path)
plan_keys  = wdb.fetch_confirmed_plan(conn, hour[:10])
open_tasks = {t["task_key"]: t for t in wdb.fetch_open_tasks(conn)}
conn.close()

ctx = HourContext(hour=hour, db_path=db_path, server_url=server)
ctx.report = report

def make_cand(key):
    t = open_tasks.get(key, {"task_key": key, "title": key})
    return Candidate(task_key=key, title=t.get("title", key), doc=wdb.render_doc(t))

# ── Tier 1 ──────────────────────────────────────────────────────────────
bindings = []
if plan_keys:
    ctx.daily = [make_cand(k) for k in plan_keys if k in open_tasks]
    print(f"\n→ tier-1: {len(ctx.daily)} daily-plan candidates")
    for c in ctx.daily:
        print(f"   {c.task_key}: {c.title}")
    print("→ calling LLM (tier-1) …")
    bindings = classify_tier1(ctx.server_url, ctx.report, ctx.daily)
else:
    print(f"\n→ tier-1: no confirmed daily plan for {hour[:10]} — skipping")

# ── Tier 2 ──────────────────────────────────────────────────────────────
if len(bindings) < 2:
    backlog_keys = [k for k in open_tasks if k not in set(plan_keys)]
    ctx.backlog = [make_cand(k) for k in backlog_keys][:15]
    print(f"\n→ tier-2: {len(ctx.backlog)} backlog candidates")
    for i in range(0, len(ctx.backlog), BATCH):
        batch = ctx.backlog[i:i + BATCH]
        print(f"→ calling LLM (tier-2 batch {i // BATCH}) …")
        bindings = classify_tier2_batch(ctx.server_url, ctx.report, batch, i // BATCH)
        if bindings:
            break

# ── Result ──────────────────────────────────────────────────────────────
tier = 1 if plan_keys and bindings else 2
print("\n════════════════════════════════════════")
print(f"  Classification Result — {hour}")
print("════════════════════════════════════════")
if bindings:
    print(f"  tier-{tier} matched:")
    for b in bindings:
        print(f"  ✓  {b.task_key}  (conf={b.confidence:.2f})")
        print(f"     {b.why}")
else:
    print("  NO MATCH — pipeline would propose a new ticket")
PYEOF
