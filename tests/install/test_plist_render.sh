#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions
set -uo pipefail
TESTS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${REPO_ROOT:-$(cd "${TESTS_DIR}/../.." && pwd)}"
# shellcheck source=lib.sh
source "${TESTS_DIR}/lib.sh"

# We simulate the sed substitutions from install-daemon.sh and
# install-screenpipe-daemon.sh without invoking launchctl. This verifies that
# every {{PLACEHOLDER}} in the plist templates is covered by the install
# scripts' sed commands, leaving a valid plist with no leftover tokens.

TMPDIR_RENDER="$(mktemp -d)"
trap 'rm -rf "$TMPDIR_RENDER"' EXIT

# --- daemon plist ---
DAEMON_PLIST="${REPO_ROOT}/scripts/com.meridiona.daemon.plist"
RENDERED_DAEMON="${TMPDIR_RENDER}/com.meridiona.daemon.rendered.plist"

sed \
    -e "s|{{REPO_ROOT}}|/tmp/test-repo|g" \
    -e "s|{{HOME}}|/tmp/test-home|g" \
    -e "s|{{DAEMON_BIN}}|/tmp/test-home/.local/bin/meridian-daemon|g" \
    -e "s|{{MERIDIAN_OO_AUTH}}||g" \
    -e "s|{{MERIDIAN_OTLP_ENDPOINT}}||g" \
    "$DAEMON_PLIST" > "$RENDERED_DAEMON"

start_test "daemon plist: no leftover {{placeholders}} after render"
_leftover_count="$(grep -c '{{' "$RENDERED_DAEMON" || true)"
assert_eq "0" "$_leftover_count" "no leftover placeholders"

start_test "daemon plist: rendered plist passes plutil -lint"
assert_ok "plutil -lint on rendered daemon plist" plutil -lint "$RENDERED_DAEMON"

# --- screenpipe plist ---
SCREENPIPE_PLIST="${REPO_ROOT}/scripts/com.meridiona.screenpipe.plist"
RENDERED_SCREENPIPE="${TMPDIR_RENDER}/com.meridiona.screenpipe.rendered.plist"

sed \
    -e "s|{{HOME}}|/tmp/test-home|g" \
    -e "s|{{SCREENPIPE_BIN}}|/opt/homebrew/bin/screenpipe|g" \
    "$SCREENPIPE_PLIST" > "$RENDERED_SCREENPIPE"

start_test "screenpipe plist: no leftover {{placeholders}} after render"
_leftover_count="$(grep -c '{{' "$RENDERED_SCREENPIPE" || true)"
assert_eq "0" "$_leftover_count" "no leftover placeholders"

start_test "screenpipe plist: rendered plist passes plutil -lint"
assert_ok "plutil -lint on rendered screenpipe plist" plutil -lint "$RENDERED_SCREENPIPE"

# --- ui plist ---
UI_PLIST="${REPO_ROOT}/scripts/com.meridiona.ui.plist"
RENDERED_UI="${TMPDIR_RENDER}/com.meridiona.ui.rendered.plist"

sed \
    -e "s|{{REPO_ROOT}}|/tmp/test-repo|g" \
    -e "s|{{HOME}}|/tmp/test-home|g" \
    -e "s|{{NPM_BIN}}|/opt/homebrew/bin/npm|g" \
    "$UI_PLIST" > "$RENDERED_UI"

start_test "ui plist: no leftover {{placeholders}} after render"
_leftover_count="$(grep -c '{{' "$RENDERED_UI" || true)"
assert_eq "0" "$_leftover_count" "no leftover placeholders"

start_test "ui plist: rendered plist passes plutil -lint"
assert_ok "plutil -lint on rendered ui plist" plutil -lint "$RENDERED_UI"

exit "$FAIL_COUNT"
