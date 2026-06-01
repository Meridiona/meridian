#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions
#
# Bridge from the npm install to the real install. Copies the prebuilt bundle
# (the @meridiona/meridian-darwin-arm64 package contents) into ~/.meridian/app
# and runs the bundle installer there — so the daemons run from a stable
# location, not npm's volatile global node_modules.
#
#   meridian-npm-setup.sh <bundle_dir> [--skip-permissions]
set -euo pipefail

BUNDLE="${1:?usage: meridian-npm-setup.sh <bundle_dir> [args…]}"
shift || true
APP="${HOME}/.meridian/app"

[[ -x "${BUNDLE}/bin/meridian" ]] || { echo "✗ bundle at ${BUNDLE} is missing bin/meridian" >&2; exit 1; }

mkdir -p "$(dirname "${APP}")"
# Preserve an existing .env across re-installs/updates.
keep=""
if [[ -f "${APP}/.env" ]]; then keep="$(mktemp)"; cp "${APP}/.env" "${keep}"; fi

rm -rf "${APP}"
mkdir -p "${APP}"
# Copy the prebuilt payload (bin/ ui/ services/ scripts/ .env.example VERSION).
cp -R "${BUNDLE}/." "${APP}/"
# Drop npm-package metadata that isn't part of the app.
rm -f "${APP}/package.json" "${APP}/README.md" "${APP}/.gitignore" "${APP}/.npmignore"

[[ -n "${keep}" ]] && { cp "${keep}" "${APP}/.env"; rm -f "${keep}"; }

exec bash "${APP}/scripts/install-from-bundle.sh" "$@"
