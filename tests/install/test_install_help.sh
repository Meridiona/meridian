#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
set -uo pipefail
TESTS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${REPO_ROOT:-$(cd "${TESTS_DIR}/../.." && pwd)}"
# shellcheck source=lib.sh
source "${TESTS_DIR}/lib.sh"

start_test "install.sh --help exits 0"
assert_ok "install.sh --help exits 0" bash "${REPO_ROOT}/install.sh" --help

start_test "install.sh --help mentions --no-ui"
assert_stdout_matches "--no-ui in help output" '--no-ui' bash "${REPO_ROOT}/install.sh" --help

start_test "install.sh --help mentions --dry-run"
assert_stdout_matches "--dry-run in help output" '--dry-run' bash "${REPO_ROOT}/install.sh" --help

start_test "install.sh --help mentions --no-daemon"
assert_stdout_matches "--no-daemon in help output" '--no-daemon' bash "${REPO_ROOT}/install.sh" --help

start_test "install.sh --help mentions --skip-permissions"
assert_stdout_matches "--skip-permissions in help output" '--skip-permissions' bash "${REPO_ROOT}/install.sh" --help

start_test "install.sh --help mentions --skip-env"
assert_stdout_matches "--skip-env in help output" '--skip-env' bash "${REPO_ROOT}/install.sh" --help

exit "$FAIL_COUNT"
