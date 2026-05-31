#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions
#
# One-command installer. Downloads the latest prebuilt macOS arm64 release,
# unpacks it to ~/.meridian/app, and runs the bundle installer. No git clone,
# no cargo/npm build.
#
#   curl -fsSL https://raw.githubusercontent.com/Meridiona/meridian/main/bootstrap.sh | bash
#
# Options (env):
#   MERIDIAN_VERSION=v0.3.0   install a specific tag instead of latest
#   MERIDIAN_SKIP_PERMISSIONS=1   skip the macOS permissions walkthrough
set -euo pipefail

REPO="Meridiona/meridian"
APP="${HOME}/.meridian/app"

err() { echo "✗ $*" >&2; exit 1; }

[[ "$(uname -s)" == "Darwin" ]] || err "Meridian requires macOS."
[[ "$(uname -m)" == "arm64" ]]  || err "Meridian requires Apple Silicon (arm64)."
command -v curl >/dev/null 2>&1 || err "curl is required."

# Resolve the release + the macOS-arm64 asset URL (unauthenticated GitHub API).
if [[ -n "${MERIDIAN_VERSION:-}" ]]; then
    api="https://api.github.com/repos/${REPO}/releases/tags/${MERIDIAN_VERSION}"
else
    api="https://api.github.com/repos/${REPO}/releases/latest"
fi
echo "→ Finding the latest Meridian release…"
meta="$(curl -fsSL "${api}")" || err "could not reach the GitHub releases API for ${REPO}."
asset_url="$(printf '%s' "${meta}" \
    | grep -oE '"browser_download_url"[[:space:]]*:[[:space:]]*"[^"]*macos-arm64\.tar\.gz"' \
    | head -1 | sed -E 's/.*"(https[^"]+)"/\1/')"
[[ -n "${asset_url}" ]] || err "no macOS-arm64 release asset found. Has a release been published yet?"
tag="$(printf '%s' "${meta}" | grep -oE '"tag_name"[[:space:]]*:[[:space:]]*"[^"]*"' | head -1 | sed -E 's/.*"([^"]+)"$/\1/')"

tmp="$(mktemp -d)"
trap 'rm -rf "${tmp}"' EXIT
echo "→ Downloading ${tag:-release} …"
curl -fSL "${asset_url}" -o "${tmp}/meridian.tar.gz" || err "download failed."
tar xzf "${tmp}/meridian.tar.gz" -C "${tmp}" || err "could not unpack the release tarball."
src="$(find "${tmp}" -maxdepth 1 -type d -name 'meridian-*-macos-arm64' | head -1)"
[[ -n "${src}" ]] || err "unexpected tarball layout."

echo "→ Installing to ${APP} …"
mkdir -p "$(dirname "${APP}")"
# Preserve the user's .env across re-installs.
[[ -f "${APP}/.env" ]] && cp "${APP}/.env" "${tmp}/.env.keep"
rm -rf "${APP}"
mv "${src}" "${APP}"
[[ -f "${tmp}/.env.keep" ]] && cp "${tmp}/.env.keep" "${APP}/.env"

args=()
[[ "${MERIDIAN_SKIP_PERMISSIONS:-0}" == "1" ]] && args+=(--skip-permissions)
exec bash "${APP}/scripts/install-from-bundle.sh" "${args[@]}"
