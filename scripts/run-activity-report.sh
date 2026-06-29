#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
# Run distill_hour → activity_report for a given local hour label.
#
# Usage:
#   bash scripts/run-activity-report.sh [HOUR]
#
# HOUR: local hour label in YYYY-MM-DDTHH format (your system's local timezone).
#       Defaults to the current local hour if omitted.
#
# Example:
#   bash scripts/run-activity-report.sh 2026-06-28T13
#
# Traces and logs appear in OpenObserve because the MLX server is started
# with MERIDIAN_OO_EXPORT=1 (set in dev-start.sh).

set -euo pipefail

SERVER="http://127.0.0.1:7823"
DB_PATH="${MERIDIAN_DB:-$HOME/.meridian/meridian.db}"

# Default to current local hour
HOUR="${1:-$(date '+%Y-%m-%dT%H')}"

echo "→ hour: $HOUR"
echo "→ db:   $DB_PATH"
echo ""

# --- 1. Check server is up ---
if ! curl -sf "$SERVER/health" >/dev/null 2>&1; then
    echo "✗ MLX server not running at $SERVER — start it with dev-start.sh" >&2
    exit 1
fi

# --- 2. Distill ---
echo "→ distilling sessions for $HOUR …"
DISTILL_FILE=$(mktemp /tmp/meridian-distill-XXXXXX)
curl -sf -X POST "$SERVER/distill_hour" \
    -H "Content-Type: application/json" \
    -d "{\"hour\":\"$HOUR\",\"db_path\":\"$DB_PATH\"}" \
    -o "$DISTILL_FILE"

NSESS=$(python3 -c "import json,sys; d=json.load(open('$DISTILL_FILE')); print(d['nsess'])")
BODY_CHARS=$(python3 -c "import json,sys; d=json.load(open('$DISTILL_FILE')); print(len(d['body']))")

echo "   sessions: $NSESS  body: ${BODY_CHARS} chars"

if [ "$NSESS" -eq 0 ]; then
    echo "✗ no sessions found for $HOUR — nothing to report"
    rm -f "$DISTILL_FILE"
    exit 0
fi

# --- 3. Activity report ---
echo "→ running activity_report (this takes ~1–4 min) …"
REPORT_FILE=$(mktemp /tmp/meridian-report-XXXXXX)

python3 - "$DISTILL_FILE" "$REPORT_FILE" "$DB_PATH" <<'PYEOF'
import json, sys, urllib.request

distill  = json.load(open(sys.argv[1]))
db_path  = sys.argv[3]
payload = json.dumps({
    "body":     distill["body"],
    "label":    distill["label"],
    "db_path":  db_path,
}).encode()

req = urllib.request.Request(
    "http://127.0.0.1:7823/activity_report",
    data=payload,
    headers={"Content-Type": "application/json"},
    method="POST",
)
with urllib.request.urlopen(req, timeout=600) as resp:
    result = json.load(resp)

json.dump(result, open(sys.argv[2], "w"))
print(f"in_tok={result['input_tokens']} out_tok={result['output_tokens']} "
      f"think_tok={result['think_tokens']} elapsed={result['elapsed_s']}s")
PYEOF

# --- 4. Print report ---
echo ""
echo "════════════════════════════════════════"
echo "  Activity Report — $HOUR"
echo "════════════════════════════════════════"
python3 -c "import json; print(json.load(open('$REPORT_FILE'))['report'])"

rm -f "$DISTILL_FILE" "$REPORT_FILE"
