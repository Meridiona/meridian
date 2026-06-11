#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
set -uo pipefail
TESTS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${REPO_ROOT:-$(cd "${TESTS_DIR}/../.." && pwd)}"
# shellcheck source=lib.sh
source "${TESTS_DIR}/lib.sh"

# doctor is diagnostic — it must always exit 0 regardless of check outcomes.
start_test "meridian-cli.sh doctor exits 0"
assert_ok "doctor exits 0 (diagnostic, never fatal)" \
    bash "${REPO_ROOT}/scripts/meridian-cli.sh" doctor

start_test "doctor output mentions macOS"
assert_stdout_matches "doctor mentions macOS" \
    'macOS' bash "${REPO_ROOT}/scripts/meridian-cli.sh" doctor

start_test "doctor output prints a final summary"
assert_stdout_matches "doctor prints checks-passed or checks-failed summary" \
    'checks passed|check(s)? failed' bash "${REPO_ROOT}/scripts/meridian-cli.sh" doctor

exit "$FAIL_COUNT"
