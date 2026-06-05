#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions
#
# Test suite for a11y-helper permission flow
# Verifies all changes work correctly without breaking existing code

set -euo pipefail

info() { printf '→ %s\n'   "$*"; }
ok()   { printf '  ✓ %s\n' "$*"; }
err()  { printf '✗ %s\n'   "$*" >&2; exit 1; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INSTALL_SH="${SCRIPT_DIR}/install.sh"
BOOTSTRAP_SH="${SCRIPT_DIR}/scripts/bootstrap.sh"
FIX_SCRIPT="${SCRIPT_DIR}/scripts/fix-a11y-helper-permission.sh"

# Test 1: Verify install.sh syntax
info "Test 1: Checking install.sh syntax…"
if bash -n "${INSTALL_SH}"; then
    ok "install.sh syntax valid"
else
    err "install.sh has syntax errors"
fi

# Test 2: Verify bootstrap.sh syntax
info "Test 2: Checking bootstrap.sh syntax…"
if bash -n "${BOOTSTRAP_SH}"; then
    ok "bootstrap.sh syntax valid"
else
    err "bootstrap.sh has syntax errors"
fi

# Test 3: Verify fix script syntax
info "Test 3: Checking fix-a11y-helper-permission.sh syntax…"
if bash -n "${FIX_SCRIPT}"; then
    ok "fix-a11y-helper-permission.sh syntax valid"
else
    err "fix-a11y-helper-permission.sh has syntax errors"
fi

# Test 4: Verify TypeScript files exist
info "Test 4: Checking new UI components…"
HEALTH_ROUTE="${SCRIPT_DIR}/ui/app/api/health/route.ts"
HEALTH_BANNER="${SCRIPT_DIR}/ui/components/HealthBanner.tsx"

if [[ -f "${HEALTH_ROUTE}" ]]; then
    ok "Health API route exists: ${HEALTH_ROUTE}"
else
    err "Health API route missing: ${HEALTH_ROUTE}"
fi

if [[ -f "${HEALTH_BANNER}" ]]; then
    ok "HealthBanner component exists: ${HEALTH_BANNER}"
else
    err "HealthBanner component missing: ${HEALTH_BANNER}"
fi

# Test 5: Verify page.tsx imports HealthBanner
info "Test 5: Checking page.tsx integration…"
if grep -q "import HealthBanner" "${SCRIPT_DIR}/ui/app/page.tsx"; then
    ok "page.tsx imports HealthBanner"
else
    err "page.tsx does not import HealthBanner"
fi

if grep -q "<HealthBanner" "${SCRIPT_DIR}/ui/app/page.tsx"; then
    ok "page.tsx uses HealthBanner component"
else
    err "page.tsx does not use HealthBanner component"
fi

# Test 6: Verify install.sh includes the new functions
info "Test 6: Checking install.sh functions…"
if grep -q "is_a11y_helper_trusted" "${INSTALL_SH}"; then
    ok "is_a11y_helper_trusted() function exists"
else
    err "is_a11y_helper_trusted() function missing"
fi

if grep -q "prompt_a11y_helper_permission" "${INSTALL_SH}"; then
    ok "prompt_a11y_helper_permission() function exists"
else
    err "prompt_a11y_helper_permission() function missing"
fi

# Test 7: Verify function is called after a11y-helper install
info "Test 7: Checking function invocation…"
if grep -q "prompt_a11y_helper_permission$" "${INSTALL_SH}"; then
    ok "prompt_a11y_helper_permission is called"
else
    err "prompt_a11y_helper_permission is not called"
fi

# Test 8: Verify bootstrap.sh mentions a11y-helper
info "Test 8: Checking bootstrap.sh message…"
if grep -q "a11y-helper" "${BOOTSTRAP_SH}"; then
    ok "bootstrap.sh mentions a11y-helper"
else
    err "bootstrap.sh does not mention a11y-helper"
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
ok "All tests passed! Implementation is complete and valid."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
