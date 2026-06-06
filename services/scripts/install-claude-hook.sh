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
# Resolution order: explicit MERIDIAN_BIN override (bundle installer passes
# the installed binary), the release build in this repo, the bundle install
# location, then whatever is on PATH.
if [[ -n "${MERIDIAN_BIN:-}" && -x "${MERIDIAN_BIN}" ]]; then
    : # caller pinned the binary
elif [[ -x "${REPO_ROOT}/target/release/meridian" ]]; then
    MERIDIAN_BIN="${REPO_ROOT}/target/release/meridian"
elif [[ -x "${HOME}/.meridian/app/bin/meridian" ]]; then
    MERIDIAN_BIN="${HOME}/.meridian/app/bin/meridian"
elif command -v meridian >/dev/null 2>&1; then
    MERIDIAN_BIN="$(command -v meridian)"
elif command -v meridian-daemon >/dev/null 2>&1; then
    MERIDIAN_BIN="$(command -v meridian-daemon)"
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

new_entry = {
    "hooks": [
        {
            "type":    "command",
            "command": hook_cmd,
            # Claude Code reads `timeout` in milliseconds. 30s ceiling
            # — the hook itself returns in <100 ms.
            "timeout": 30000,
        }
    ]
}

# Replace any prior meridian entry by matching on the command substring
# "coding-agent-hook". Claude Code strips unknown JSON fields (like the
# former "_meridian" marker) on every save, so command-string matching
# is the only reliable idempotency mechanism.
#
# "coding_agent_indexer" matches the retired Python hook (`python -m
# coding_agent_indexer.hook`) — the package was removed when the indexer
# moved into the Rust daemon, so stale entries just error on every
# SessionEnd. Purge them on upgrade.
OUR_MARKERS = ("coding-agent-hook", "coding_agent_indexer")
filtered = []
removed = 0
for group in session_end:
    is_ours = any(
        marker in h.get("command", "")
        for h in group.get("hooks", [])
        for marker in OUR_MARKERS
    )
    if is_ours:
        removed += 1
    else:
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
