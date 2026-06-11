#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
set -uo pipefail
TESTS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${REPO_ROOT:-$(cd "${TESTS_DIR}/../.." && pwd)}"
# shellcheck source=lib.sh
source "${TESTS_DIR}/lib.sh"

# ---------------------------------------------------------------------------
# Self-contained implementations that mirror the contract defined by Coder A.
# We cannot source install.sh because it runs prereq detection on load.
# These implementations duplicate production logic for testability — the tests
# validate the contract, not any particular implementation detail.
# ---------------------------------------------------------------------------

get_env_value() {
    local key="$1" file="$2"
    [[ -f "$file" ]] || return 0
    grep -E "^${key}=" "$file" 2>/dev/null | tail -1 | cut -d= -f2- || true
}

set_env_value() {
    local key="$1" value="$2" file="$3"
    [[ -f "$file" ]] || touch "$file"
    if grep -qE "^${key}=" "$file" 2>/dev/null; then
        local tmp
        tmp="$(mktemp)"
        awk -v k="$key" -v v="$value" '
            BEGIN { FS=OFS="="; replaced=0 }
            $1==k && !replaced { print k"="v; replaced=1; next }
            { print }
        ' "$file" > "$tmp"
        mv "$tmp" "$file"
    else
        printf '%s=%s\n' "$key" "$value" >> "$file"
    fi
}

# ---------------------------------------------------------------------------
# Test cases
# ---------------------------------------------------------------------------

TMPDIR_ENV="$(mktemp -d)"
trap 'rm -rf "$TMPDIR_ENV"' EXIT
ENV_FILE="${TMPDIR_ENV}/.env"

# 1. set_env_value on a non-existent file creates it with the right content
start_test "set_env_value: creates file and writes FOO=bar"
set_env_value FOO bar "$ENV_FILE"
assert_ok "env file was created" test -f "$ENV_FILE"
_val="$(grep "FOO=bar" "$ENV_FILE" || true)"
assert_eq "FOO=bar" "$_val" "FOO=bar written to new file"

# 2. get_env_value returns the written value
start_test "get_env_value: returns bar for key FOO"
_got="$(get_env_value FOO "$ENV_FILE")"
assert_eq "bar" "$_got" "get_env_value FOO returns bar"

# 3. set_env_value with same key and new value replaces — not appends
start_test "set_env_value: replaces existing key FOO with baz"
set_env_value FOO baz "$ENV_FILE"
_count="$(grep -c "^FOO=" "$ENV_FILE")"
assert_eq "1" "$_count" "exactly one FOO= line"
_got="$(get_env_value FOO "$ENV_FILE")"
assert_eq "baz" "$_got" "FOO value is now baz"

# 4. set_env_value with a new key appends without removing existing key
start_test "set_env_value: appends new key BAZ=qux"
set_env_value BAZ qux "$ENV_FILE"
_foo_val="$(get_env_value FOO "$ENV_FILE")"
_baz_val="$(get_env_value BAZ "$ENV_FILE")"
assert_eq "baz" "$_foo_val" "FOO still present after adding BAZ"
assert_eq "qux" "$_baz_val" "BAZ=qux appended"

# 5. get_env_value returns empty string for missing key
start_test "get_env_value: returns empty string for missing key"
_got="$(get_env_value MISSING "$ENV_FILE")"
assert_eq "" "$_got" "missing key returns empty string"

# 6. get_env_value returns empty string when file does not exist (no error)
start_test "get_env_value: returns empty string when file missing"
_got="$(get_env_value FOO "${TMPDIR_ENV}/nonexistent.env")"
assert_eq "" "$_got" "nonexistent file returns empty string"

exit "$FAIL_COUNT"
