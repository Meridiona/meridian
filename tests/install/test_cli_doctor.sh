#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
set -uo pipefail
TESTS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${REPO_ROOT:-$(cd "${TESTS_DIR}/../.." && pwd)}"
# shellcheck source=lib.sh
source "${TESTS_DIR}/lib.sh"

# The wrapper fallback is intentionally diagnostic and must remain useful even
# without a compiled native binary. A native doctor run may exit 1 when it finds
# a critical fault; the wrapper treats that as a valid doctor result rather than
# a stale-binary timeout.
start_test "doctor output mentions macOS"
assert_stdout_matches "doctor mentions macOS" \
    'macOS' bash "${REPO_ROOT}/scripts/meridian-cli.sh" doctor

start_test "doctor output contains a health verdict or fallback next step"
assert_stdout_matches "doctor prints a current health summary" \
    'all systems healthy|healthy with warnings|critical issues|next step' \
    bash "${REPO_ROOT}/scripts/meridian-cli.sh" doctor

exit "$FAIL_COUNT"
