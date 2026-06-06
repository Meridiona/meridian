#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions
#
# Builds the meridian-a11y-helper binary from main.swift and ad-hoc signs it
# with a stable identifier.
#
# ⚠ DO NOT wire this into CI or package-release.sh. The built binary is
# COMMITTED to the repo on purpose: users grant it the Accessibility
# permission, and macOS keys that grant to the binary's code hash (CDHash).
# A byte-identical binary keeps the grant across meridian updates; a rebuilt
# one silently loses it on every release. Rebuild ONLY when main.swift
# changes, commit the new binary, and call out the required permission
# re-grant in the release notes.
set -euo pipefail
cd "$(dirname "$0")"

# Guard: rebuilding changes the binary's CDHash, which silently revokes every
# user's Accessibility TCC grant for this helper. Only proceed when the caller
# has confirmed they understand and will include a re-grant notice in the
# release notes.
echo "⚠  WARNING: rebuilding meridian-a11y-helper changes its CDHash." >&2
echo "   Every user's macOS Accessibility grant for this binary will be" >&2
echo "   silently REVOKED on their next screenpipe restart. You MUST" >&2
echo "   document the re-grant step in the release notes." >&2
echo "" >&2
read -r -p "   Type 'yes' to confirm you understand and want to continue: " _confirm
if [[ "${_confirm}" != "yes" ]]; then
    echo "Aborted." >&2
    exit 1
fi

swiftc -O -o meridian-a11y-helper main.swift
codesign -s - -f --identifier com.meridiona.a11y-helper meridian-a11y-helper
echo "built + signed: $(pwd)/meridian-a11y-helper"
codesign -dv meridian-a11y-helper 2>&1 | grep -E "Identifier|CodeDirectory"
