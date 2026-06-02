#!/usr/bin/env bash
# Install a Claude Code SessionEnd hook that registers each ended
# Claude session as an app_sessions row in meridian.db in real time.
#
# Pairs with the in-daemon coding-agent indexer (Rust): the daemon does
# a fallback poll for crashes / Codex / sleep gaps; this hook handles the
# 99 % happy path with zero latency by invoking `meridian coding-agent-hook`.
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
REPO_ROOT="$(cd "${SERVICES_DIR}/.." && pwd)"

# Locate the meridian binary that owns the `coding-agent-hook` subcommand.
# In a bundle install the real binary is at REPO_ROOT/bin/meridian (not the
# Node.js launcher on PATH, which has no `coding-agent-hook` command).
# Priority: bundle binary → source build → PATH meridian-daemon → PATH meridian.
if [[ -x "${REPO_ROOT}/bin/meridian" ]]; then
    MERIDIAN_BIN="${REPO_ROOT}/bin/meridian"
elif [[ -x "${REPO_ROOT}/target/release/meridian" ]]; then
    MERIDIAN_BIN="${REPO_ROOT}/target/release/meridian"
elif command -v meridian-daemon >/dev/null 2>&1; then
    MERIDIAN_BIN="$(command -v meridian-daemon)"
elif command -v meridian >/dev/null 2>&1; then
    MERIDIAN_BIN="$(command -v meridian)"
else
    echo "✗ meridian binary not found — build it first: cargo build --release" >&2
    exit 1
fi
echo "→ using meridian binary: ${MERIDIAN_BIN}"

# Python is only used here as the install-time settings.json editor (stdlib
# json); it is NOT part of the installed hook command itself.
PYTHON="$(command -v python3)"

SETTINGS="${HOME}/.claude/settings.json"
mkdir -p "$(dirname "${SETTINGS}")"
[[ -f "${SETTINGS}" ]] || echo '{}' > "${SETTINGS}"

# Marker we use to identify "our" hook entry on re-install / uninstall.
MARKER="meridian:coding-agent-indexer:SessionEnd"

# The shell command the hook fires: exec the meridian binary's
# `coding-agent-hook` subcommand — stdin = the SessionEnd JSON payload
# from Claude (it reads `transcript_path` off stdin and seals one session).
#
# The path is shell-escaped with `printf %q` so the command survives
# spaces / shell metacharacters in the repo install location when
# Claude Code invokes it via `bash -c`.
HOOK_CMD=$(printf 'exec %q coding-agent-hook' "${MERIDIAN_BIN}")

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
echo "Live test (will seal one session + log):"
echo "  echo '{\"transcript_path\":\"~/.claude/projects/.../<uuid>.jsonl\",\"hook_event_name\":\"SessionEnd\"}' | bash -c '${HOOK_CMD}'"
echo
echo "Uninstall:"
echo "  ${SCRIPT_DIR}/uninstall-claude-hook.sh"
