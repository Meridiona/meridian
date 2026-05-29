#!/usr/bin/env bash
# Install a Claude Code SessionEnd hook that registers each ended
# Claude session as an app_sessions row in meridian.db in real time.
#
# Pairs with the coding-agent-indexer daemon: the daemon does a
# fallback poll every 10 min for crashes / Codex / sleep gaps; this
# hook handles the 99 % happy path with zero latency.
#
# This script MERGES the entry into ~/.claude/settings.json without
# touching any other hooks you've configured. Re-running is idempotent:
# the existing meridian SessionEnd entry is replaced, others untouched.
#
#   ./services/scripts/install-claude-hook.sh
#
# Uninstall:
#   ./services/scripts/uninstall-claude-hook.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SERVICES_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

# Pick the same python the launchd daemon uses so deps resolve identically.
VENV313_PYTHON="${SERVICES_DIR}/.venv313/bin/python"
VENV_PYTHON="${SERVICES_DIR}/.venv/bin/python"
if [[ -x "${VENV313_PYTHON}" ]]; then
    PYTHON="${VENV313_PYTHON}"
elif [[ -x "${VENV_PYTHON}" ]]; then
    PYTHON="${VENV_PYTHON}"
else
    PYTHON="$(command -v python3)"
fi
echo "→ using python: ${PYTHON}"

SETTINGS="${HOME}/.claude/settings.json"
mkdir -p "$(dirname "${SETTINGS}")"
[[ -f "${SETTINGS}" ]] || echo '{}' > "${SETTINGS}"

# Marker we use to identify "our" hook entry on re-install / uninstall.
MARKER="meridian:coding-agent-indexer:SessionEnd"

# The shell command the hook fires. We:
#   - cd into services/  so `coding_agent_indexer.hook` imports
#   - set PYTHONPATH defensively in case cd fails
#   - exec the python module — stdin = the SessionEnd JSON payload from Claude
#
# Paths are shell-escaped with `printf %q` so the command survives
# spaces / shell metacharacters in the repo install location when
# Claude Code invokes it via `bash -c`.
HOOK_CMD=$(printf 'cd %q && PYTHONPATH=%q exec %q -m coding_agent_indexer.hook' \
    "${SERVICES_DIR}" "${SERVICES_DIR}" "${PYTHON}")

echo "→ merging SessionEnd hook into ${SETTINGS}"
"${PYTHON}" - "${SETTINGS}" "${HOOK_CMD}" "${MARKER}" <<'PYEOF'
import json, sys, os, tempfile

settings_path, hook_cmd, marker = sys.argv[1:4]
with open(settings_path) as f:
    settings = json.load(f)

hooks = settings.setdefault("hooks", {})
session_end = hooks.setdefault("SessionEnd", [])

# SessionEnd doesn't support `matcher` — each entry is just a list of
# hooks. We use an unmatched group whose first command carries our
# marker as a comment so we can find + replace it later.
new_entry = {
    "hooks": [
        {
            "type":    "command",
            "command": hook_cmd,
            # Claude Code reads `timeout` in milliseconds (matching the
            # existing entries already in this settings.json). 30s ceiling
            # — the hook itself returns in <100 ms.
            "timeout": 30000,
            # Comment field that Claude Code preserves but doesn't act on
            # — gives us a robust idempotency / uninstall handle.
            "_meridian": marker,
        }
    ]
}

# Replace any prior meridian entry, leave all others untouched.
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

filtered.append(new_entry)
hooks["SessionEnd"] = filtered

# Atomic write so a crash mid-rewrite never corrupts settings.json.
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

print(f"replaced {removed} prior meridian entr{'y' if removed == 1 else 'ies'}; "
      f"total SessionEnd entries now: {len(filtered)}")
PYEOF

echo
echo "✓ Claude Code SessionEnd hook installed"
echo
echo "Verify:"
echo "  python3 -c \"import json; print(json.dumps(json.load(open('${SETTINGS}'))['hooks']['SessionEnd'], indent=2))\""
echo
echo "Live test (will hit the indexer + log):"
echo "  echo '{\"transcript_path\":\"~/.claude/projects/.../<uuid>.jsonl\",\"hook_event_name\":\"SessionEnd\"}' | bash -c '${HOOK_CMD}'"
echo
echo "Uninstall:"
echo "  ${SCRIPT_DIR}/uninstall-claude-hook.sh"
