#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

# Shared helpers for the install test suite.
# Color-coded output for test reports. Tracked via FAIL_COUNT global.

FAIL_COUNT=0
TEST_NAME=""

start_test() { TEST_NAME="$1"; }
pass() { printf "  ✓ %s\n" "${1:-${TEST_NAME}}"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); printf "  ✗ %s — %s\n" "${TEST_NAME}" "$1" >&2; }

# Returns 0 if equal, else fails the running test.
assert_eq() {
    local expected="$1" actual="$2" msg="${3:-equality}"
    if [[ "$expected" == "$actual" ]]; then
        pass "$msg"
    else
        fail "$msg: expected '$expected', got '$actual'"
    fi
}

# Runs the command; passes if exit 0, fails otherwise.
assert_ok() {
    local desc="$1"; shift
    if "$@" >/dev/null 2>&1; then pass "$desc"; else fail "$desc (cmd failed: $*)"; fi
}

# Runs the command; passes if nonzero exit, fails if 0.
assert_fail() {
    local desc="$1"; shift
    if "$@" >/dev/null 2>&1; then fail "$desc (expected nonzero, got 0)"; else pass "$desc"; fi
}

# Asserts the command's stdout matches a regex (using grep -qE).
# The '--' separator prevents patterns starting with '-' from being
# misinterpreted as grep flags on macOS BSD grep.
assert_stdout_matches() {
    local desc="$1" pattern="$2"; shift 2
    local out
    out="$("$@" 2>&1)"
    if grep -qE -- "$pattern" <<< "$out"; then pass "$desc"; else fail "$desc (pattern '$pattern' not in output)"; fi
}
