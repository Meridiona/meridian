#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions
set -uo pipefail
TESTS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${REPO_ROOT:-$(cd "${TESTS_DIR}/../.." && pwd)}"
# shellcheck source=lib.sh
source "${TESTS_DIR}/lib.sh"

CLI="${REPO_ROOT}/scripts/meridian-cli.sh"

start_test "meridian-cli.sh xyzzy exits nonzero"
assert_fail "unknown command exits nonzero" bash "$CLI" xyzzy

start_test "meridian-cli.sh xyzzy mentions 'unknown command' or shows usage"
assert_stdout_matches "unknown command error text" \
    'unknown command|Usage' bash "$CLI" xyzzy

exit "$FAIL_COUNT"
