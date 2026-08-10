#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
#
# Claim ~/.meridian/daemon.sock for a DEV daemon by stopping the canonical
# launchd-managed one (com.meridiona.daemon) if a packaged install is present.
#
# WHY THIS IS ITS OWN FILE, CALLED FROM THE TERMINAL TAB
#
# dev-start.sh already did this — but it did it in the SCRIPT, while the thing
# that actually runs the daemon is a Terminal tab it spawns via `osascript`.
# Those have different lifetimes. The tab's command outlives the script, and it
# gets re-run constantly: pressing Up-Enter in that window, reopening Terminal
# with "reopen windows on login", or simply starting the tab again the next
# morning. Every one of those paths ran `cargo watch` with NO teardown, so if
# the installed daemon had come back in the meantime (a `meridian start`, a
# login, reinstalling the app) it still owned the socket.
#
# The dev daemon then hit the single-instance guard and exited — with status 0,
# because for the canonical daemon that is correct: a non-zero exit would make
# launchd's KeepAlive restart-loop it. So `cargo watch` printed
# "[Finished running. Exit status: 0]" and sat there looking healthy while the
# daemon was not running at all. Days of edits can go un-exercised that way,
# and nothing in the output says so.
#
# Making the tab call this first means the precondition travels WITH the thing
# that needs it, instead of living in a parent process that may have exited
# hours ago.
#
# Deliberately NOT part of the daemon itself: a dev build silently booting out
# the installed daemon on startup would be action at a distance, and the guard
# is load-bearing — two daemons on one meridian.db both fire the clock-aligned
# HH:03 worklog trigger and double-run the hour.
#
# Re-enable the installed daemon afterwards with: meridian start
set -euo pipefail

DAEMON_LABEL="gui/$(id -u)/com.meridiona.daemon"
INSTALLED_RE='\.meridian/bin/meridian$'

# `disable` so launchd will not re-launch it at login or on kickstart, `bootout`
# to stop the live one, then pkill BY PATH as a backstop — bootout can miss if
# KeepAlive respawns at the wrong instant. The path anchor is what keeps this
# from ever matching the DEV binary (target/debug/meridian) we are about to run.
launchctl disable "${DAEMON_LABEL}" 2>/dev/null || true
launchctl bootout "${DAEMON_LABEL}" 2>/dev/null || true
pkill -f "${INSTALLED_RE}" 2>/dev/null || true

# Give the socket a moment to be released before the caller binds it.
for _ in 1 2 3 4 5; do
    pgrep -f "${INSTALLED_RE}" >/dev/null 2>&1 || break
    sleep 0.4
done

if pgrep -f "${INSTALLED_RE}" >/dev/null 2>&1; then
    # Fail LOUD and stop the chain. The whole point of this file is that the
    # silent version of this failure is indistinguishable from success, so
    # returning 0 here would reintroduce exactly the bug it exists to prevent.
    echo "✗ an installed daemon (~/.meridian/bin/meridian) is STILL running and owns" >&2
    echo "  ~/.meridian/daemon.sock, so the dev daemon would start and immediately" >&2
    echo "  exit via the single-instance guard." >&2
    echo "  Quit the Meridian app from the menubar, then re-run." >&2
    exit 1
fi

echo "✓ daemon socket is free (installed daemon stopped + disabled; re-enable with: meridian start)"

# A RUNNING TRAY UNDOES ALL OF THE ABOVE, so say so rather than let it look
# flaky. `backend_install::register_agent` runs `launchctl enable` +
# `bootstrap` + `kickstart -k`, so a tray that decides the daemon is missing
# re-enables the very service just disabled and hands it the socket back —
# observed here as `print-disabled` reporting "enabled" barely a minute after a
# successful `disable`, which reads as the disable silently failing when it did
# not. dev-start.sh kills the tray before calling this, so the full flow is
# unaffected; only re-running the daemon tab alone hits it.
if pgrep -f 'target/(debug|release)/meridian-tray$' >/dev/null 2>&1; then
    echo "  ⚠ a dev tray is running and may re-register the installed daemon" >&2
    echo "    (backend_install::register_agent re-enables + kickstarts it)." >&2
    echo "    If the daemon exits on the single-instance guard again, quit the tray" >&2
    echo "    or use dev-start.sh, which stops both in the right order." >&2
fi
