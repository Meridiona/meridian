#!/usr/bin/env bash
# Remove the meridian coding-agent-indexer SessionEnd hook from
# ~/.claude/settings.json. Other hooks are untouched.

set -euo pipefail

SETTINGS="${HOME}/.claude/settings.json"
MARKER="meridian:coding-agent-indexer:SessionEnd"
PYTHON="$(command -v python3)"

if [[ ! -f "${SETTINGS}" ]]; then
    echo "no settings.json at ${SETTINGS} — nothing to do"
    exit 0
fi

"${PYTHON}" - "${SETTINGS}" "${MARKER}" <<'PYEOF'
import json, os, sys, tempfile

settings_path, marker = sys.argv[1:3]
with open(settings_path) as f:
    settings = json.load(f)

session_end = settings.get("hooks", {}).get("SessionEnd", []) or []

filtered = []
removed = 0
for group in session_end:
    is_ours = False
    for h in group.get("hooks", []):
        if h.get("_meridian") == marker:
            is_ours = True
            removed += 1
            break
    if not is_ours:
        filtered.append(group)

if removed == 0:
    print("no meridian SessionEnd hook found — nothing to remove")
    sys.exit(0)

if filtered:
    settings.setdefault("hooks", {})["SessionEnd"] = filtered
else:
    # Empty array → drop the key entirely
    settings.get("hooks", {}).pop("SessionEnd", None)
    if not settings.get("hooks"):
        settings.pop("hooks", None)

tmp = tempfile.NamedTemporaryFile(
    "w", dir=os.path.dirname(settings_path), prefix=".settings.", suffix=".tmp",
    delete=False,
)
try:
    json.dump(settings, tmp, indent=2)
    tmp.write("\n")
    tmp.flush()
    os.fsync(tmp.fileno())
    tmp.close()
    os.replace(tmp.name, settings_path)
except Exception:
    os.unlink(tmp.name)
    raise

print(f"✓ removed {removed} meridian SessionEnd entr{'y' if removed == 1 else 'ies'}")
PYEOF
