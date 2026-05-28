#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions
set -uo pipefail
TESTS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${REPO_ROOT:-$(cd "${TESTS_DIR}/../.." && pwd)}"
# shellcheck source=lib.sh
source "${TESTS_DIR}/lib.sh"

start_test "plutil -lint: com.meridiona.daemon.plist"
assert_ok "plutil -lint: com.meridiona.daemon.plist" \
    plutil -lint "${REPO_ROOT}/scripts/com.meridiona.daemon.plist"

start_test "plutil -lint: com.meridiona.screenpipe.plist"
assert_ok "plutil -lint: com.meridiona.screenpipe.plist" \
    plutil -lint "${REPO_ROOT}/scripts/com.meridiona.screenpipe.plist"

start_test "plutil -lint: com.meridiona.ui.plist"
assert_ok "plutil -lint: com.meridiona.ui.plist" \
    plutil -lint "${REPO_ROOT}/scripts/com.meridiona.ui.plist"

exit "$FAIL_COUNT"
