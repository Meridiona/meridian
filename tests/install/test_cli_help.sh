#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions
set -uo pipefail
TESTS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${REPO_ROOT:-$(cd "${TESTS_DIR}/../.." && pwd)}"
# shellcheck source=lib.sh
source "${TESTS_DIR}/lib.sh"

CLI="${REPO_ROOT}/scripts/meridian-cli.sh"

start_test "meridian-cli.sh --help exits 0"
assert_ok "meridian-cli.sh --help exits 0" bash "$CLI" --help

for subcmd in start stop restart status logs doctor config permissions uninstall; do
    start_test "help mentions subcommand: $subcmd"
    assert_stdout_matches "help mentions $subcmd" "$subcmd" bash "$CLI" --help
done

exit "$FAIL_COUNT"
