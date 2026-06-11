#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
set -uo pipefail
TESTS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${REPO_ROOT:-$(cd "${TESTS_DIR}/../.." && pwd)}"
# shellcheck source=lib.sh
source "${TESTS_DIR}/lib.sh"

# status is informational — it must always exit 0 even when nothing is installed.
start_test "meridian-cli.sh status exits 0"
assert_ok "status exits 0 (informational, never fatal)" \
    bash "${REPO_ROOT}/scripts/meridian-cli.sh" status

exit "$FAIL_COUNT"
