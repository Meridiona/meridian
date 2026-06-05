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

# Preserve the Python venv across updates. Rebuilding it (python -m venv + pip
# install mlx-lm/outlines/…) costs minutes; most releases don't change Python
# deps, so move it aside and restore it to the SAME absolute path (its baked-in
# shebangs stay valid). install-from-bundle.sh then only re-pips when the deps
# hash actually changes. Kept in a sibling dir under ~/.meridian so the move is
# an instant rename (same filesystem), never a cross-volume copy.
venv_keep="${HOME}/.meridian/.venv-update-keep"
rm -rf "${venv_keep}"
if [[ -d "${APP}/services/.venv" ]]; then mv "${APP}/services/.venv" "${venv_keep}"; fi

# Preserve the UI dir when the new ui.tar.gz hash matches the installed one.
# install-from-bundle.sh detects the absent tarball + present ui/ dir and skips
# re-extraction and the daemon restart, making non-UI updates instant.
_hash_file="${HOME}/.meridian/.component-hashes"
_ui_keep="${HOME}/.meridian/.ui-update-keep"
rm -rf "${_ui_keep}"
if [[ -f "${BUNDLE}/ui.tar.gz" ]] && [[ -d "${APP}/ui" ]] && [[ -f "${_hash_file}" ]]; then
    _old_ui_hash="$(grep '^ui_tarball=' "${_hash_file}" 2>/dev/null | cut -d= -f2 || true)"
    _new_ui_hash="$(shasum -a 256 "${BUNDLE}/ui.tar.gz" | cut -d' ' -f1)"
    if [[ -n "${_old_ui_hash}" && "${_new_ui_hash}" == "${_old_ui_hash}" ]]; then
        mv "${APP}/ui" "${_ui_keep}"
    fi
fi

rm -rf "${APP}"
mkdir -p "${APP}"
# Copy the prebuilt payload (bin/ ui.tar.gz services/ scripts/ .env.example VERSION).
cp -R "${BUNDLE}/." "${APP}/"
# Drop npm-package metadata that isn't part of the app.
rm -f "${APP}/package.json" "${APP}/README.md" "${APP}/.gitignore" "${APP}/.npmignore"

[[ -n "${keep}" ]] && { cp "${keep}" "${APP}/.env"; rm -f "${keep}"; }
# Restore the preserved venv (the bundle ships services/ source but no venv).
if [[ -d "${venv_keep}" ]]; then
    mkdir -p "${APP}/services"
    rm -rf "${APP}/services/.venv"
    mv "${venv_keep}" "${APP}/services/.venv"
fi
# Restore the preserved UI dir; delete the new tarball so install-from-bundle.sh
# skips extraction and the daemon restart.
if [[ -d "${_ui_keep}" ]]; then
    rm -rf "${APP}/ui"
    mv "${_ui_keep}" "${APP}/ui"
    rm -f "${APP}/ui.tar.gz"
fi

exec bash "${APP}/scripts/install-from-bundle.sh" "$@"
