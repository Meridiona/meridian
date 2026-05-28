#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions
set -uo pipefail
TESTS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${REPO_ROOT:-$(cd "${TESTS_DIR}/../.." && pwd)}"
# shellcheck source=lib.sh
source "${TESTS_DIR}/lib.sh"

DRYRUN_CMD=(bash "${REPO_ROOT}/install.sh" --dry-run --no-ui --no-daemon --skip-permissions --skip-env)

start_test "install.sh dry-run exits 0"
assert_ok "install.sh dry-run exits 0" "${DRYRUN_CMD[@]}"

start_test "install.sh dry-run prints 'Meridian installed'"
assert_stdout_matches "dry-run prints Meridian installed" \
    'Meridian installed' "${DRYRUN_CMD[@]}"

start_test "install.sh dry-run shows [DRY-RUN] prefix"
assert_stdout_matches "dry-run shows [DRY-RUN] prefix" \
    '\[DRY-RUN\]' "${DRYRUN_CMD[@]}"

exit "$FAIL_COUNT"
