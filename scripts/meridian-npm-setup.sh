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

rm -rf "${APP}"
mkdir -p "${APP}"
# Copy the prebuilt payload (bin/ ui.tar.gz services/ scripts/ .env.example VERSION).
cp -R "${BUNDLE}/." "${APP}/"
# Drop npm-package metadata that isn't part of the app.
rm -f "${APP}/package.json" "${APP}/README.md" "${APP}/.gitignore" "${APP}/.npmignore"

[[ -n "${keep}" ]] && { cp "${keep}" "${APP}/.env"; rm -f "${keep}"; }
# Restore the preserved venv only when its Python version matches the tarball's
# target (3.11). A venv from a bad release built with Python 3.14 has
# cpython-314-darwin.so extensions that Python 3.11 cannot load. Discard it so
# install-from-bundle.sh does a fresh extraction from the correct tarball.
if [[ -d "${venv_keep}" ]]; then
    _venv_py="$("${venv_keep}/bin/python" --version 2>&1 | grep -oE '3\.[0-9]+')"
    if [[ "${_venv_py}" == "3.11" ]]; then
        mkdir -p "${APP}/services"
        rm -rf "${APP}/services/.venv"
        mv "${venv_keep}" "${APP}/services/.venv"
    else
        echo "  ⚠ discarding preserved venv (Python ${_venv_py} ≠ 3.11) — will re-extract from bundle" >&2
        rm -rf "${venv_keep}"
    fi
fi

exec bash "${APP}/scripts/install-from-bundle.sh" "$@"
