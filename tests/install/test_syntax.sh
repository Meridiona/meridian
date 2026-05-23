#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions
set -uo pipefail
TESTS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${REPO_ROOT:-$(cd "${TESTS_DIR}/../.." && pwd)}"
# shellcheck source=lib.sh
source "${TESTS_DIR}/lib.sh"

SCRIPTS=(
    "${REPO_ROOT}/install.sh"
    "${REPO_ROOT}/scripts/meridian-cli.sh"
    "${REPO_ROOT}/scripts/install-daemon.sh"
    "${REPO_ROOT}/scripts/uninstall-daemon.sh"
    "${REPO_ROOT}/scripts/install-screenpipe-daemon.sh"
    "${REPO_ROOT}/scripts/uninstall-screenpipe-daemon.sh"
)

_have_shellcheck=0
command -v shellcheck >/dev/null 2>&1 && _have_shellcheck=1

for script in "${SCRIPTS[@]}"; do
    name="$(basename "$script")"
    start_test "bash -n: $name"
    assert_ok "bash -n: $name" bash -n "$script"

    if [[ "$_have_shellcheck" -eq 1 ]]; then
        start_test "shellcheck -S error: $name"
        assert_ok "shellcheck -S error: $name" shellcheck -S error "$script"
    fi
done

exit "$FAIL_COUNT"
