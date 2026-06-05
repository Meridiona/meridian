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

swiftc -O -o meridian-a11y-helper main.swift
codesign -s - -f --identifier com.meridiona.a11y-helper meridian-a11y-helper
echo "built + signed: $(pwd)/meridian-a11y-helper"
codesign -dv meridian-a11y-helper 2>&1 | grep -E "Identifier|CodeDirectory"
