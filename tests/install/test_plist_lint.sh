#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
set -uo pipefail
TESTS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${REPO_ROOT:-$(cd "${TESTS_DIR}/../.." && pwd)}"
# shellcheck source=lib.sh
source "${TESTS_DIR}/lib.sh"

start_test "plutil -lint: com.meridiona.daemon.plist"
assert_ok "plutil -lint: com.meridiona.daemon.plist" \
    plutil -lint "${REPO_ROOT}/scripts/com.meridiona.daemon.plist"

# The screenpipe plist was removed with the in-process capture cutover (v1.64.0).
# Nothing installs that agent anymore, so there is no template left to lint.

exit "$FAIL_COUNT"
