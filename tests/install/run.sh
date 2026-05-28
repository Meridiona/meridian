#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions
set -euo pipefail
TESTS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${TESTS_DIR}/../.." && pwd)"
export REPO_ROOT

TOTAL_FAIL=0
for test_file in "${TESTS_DIR}"/test_*.sh; do
    echo "── $(basename "$test_file") ──"
    set +e
    bash "$test_file"
    rc=$?
    set -e
    TOTAL_FAIL=$((TOTAL_FAIL + rc))
done

echo
if [[ "$TOTAL_FAIL" -eq 0 ]]; then
    echo "✓ all install tests passed"
else
    echo "✗ ${TOTAL_FAIL} install test(s) failed"
fi
exit "$TOTAL_FAIL"
