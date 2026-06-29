#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
# Distill app sessions for a given local hour label and print the result.
#
# Usage:
#   bash scripts/run-distill.sh [HOUR] [--exclude-coding]
#
# HOUR: local hour label in YYYY-MM-DDTHH format (your system's local timezone).
#       Defaults to the current local hour if omitted.
#
# --exclude-coding: omit coding-agent sessions from the distilled body.
#
# Examples:
#   bash scripts/run-distill.sh
#   bash scripts/run-distill.sh 2026-06-28T13
#   bash scripts/run-distill.sh 2026-06-28T13 --exclude-coding

set -euo pipefail

SERVER="http://127.0.0.1:7823"
DB_PATH="${MERIDIAN_DB:-$HOME/.meridian/meridian.db}"
HOUR="${1:-$(date '+%Y-%m-%dT%H')}"
EXCLUDE_CODING="false"

for arg in "$@"; do
    if [ "$arg" = "--exclude-coding" ]; then
        EXCLUDE_CODING="true"
    fi
done

echo "→ hour:           $HOUR"
echo "→ db:             $DB_PATH"
echo "→ exclude-coding: $EXCLUDE_CODING"
echo ""

if ! curl -sf "$SERVER/health" >/dev/null 2>&1; then
    echo "✗ MLX server not running at $SERVER — start it with dev-start.sh" >&2
    exit 1
fi

OUT=$(curl -sf -X POST "$SERVER/distill_hour" \
    -H "Content-Type: application/json" \
    -d "{\"hour\":\"$HOUR\",\"db_path\":\"$DB_PATH\",\"exclude_coding_agent\":$EXCLUDE_CODING}")

python3 - "$OUT" <<'PYEOF'
import json, sys

d = json.loads(sys.argv[1])
print(f"sessions:    {d['nsess']}")
print(f"raw chars:   {d['raw_chars']}")
print(f"out chars:   {d['out_chars']}")
print(f"reduction:   {d['reduction_pct']:.1f}%")
print(f"elapsed:     {d['elapsed_s']}s")
print("")
if d['body']:
    print("════════════════════════════════════════")
    print(f"  Distilled body — {d['label']}")
    print("════════════════════════════════════════")
    print(d['body'])
else:
    print("(no sessions found for this hour)")
PYEOF
