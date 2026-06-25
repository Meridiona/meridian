#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
#
# Bridge from the npm install to the real install. Assembles the prebuilt bundle
# (the @meridiona/meridian-darwin-arm64 package contents) into ~/.meridian/app
# and runs the bundle installer there — so the daemons run from a stable
# location, not npm's volatile global node_modules.
#
# ATOMIC UPDATE: the new app tree is assembled COMPLETELY in a staging dir on
# the same filesystem, then swapped into place with two renames. The live app
# dir is never in a partial state — a crash anywhere during assembly leaves the
# old version untouched and running. (The previous rm-rf-then-rebuild-in-place
# flow shipped a 1.39.0 update that crashed mid-install and left machines with
# no dashboard at all.)
#
#   meridian-npm-setup.sh <bundle_dir> [--skip-permissions]
set -euo pipefail

BUNDLE="${1:?usage: meridian-npm-setup.sh <bundle_dir> [args…]}"
shift || true
APP="${HOME}/.meridian/app"
STAGE="${HOME}/.meridian/.app-staging"
OLD="${HOME}/.meridian/.app-old"

[[ -x "${BUNDLE}/bin/meridian" ]] || { echo "✗ bundle at ${BUNDLE} is missing bin/meridian" >&2; exit 1; }

mkdir -p "$(dirname "${APP}")"

# Recover from a crash between the two swap renames of a previous run (app
# already moved aside, staging not yet moved in). Microsecond window, but free
# to handle: put the old version back before assembling the new one.
if [[ ! -d "${APP}" && -d "${OLD}" ]]; then mv "${OLD}" "${APP}"; fi
rm -rf "${STAGE}" "${OLD}"

# APFS clonefile copy — instant and no extra disk on the same volume; plain
# copy elsewhere. The live tree is always CLONED into staging, never moved, so
# ~/.meridian/app stays complete until the swap.
clone_dir() { # <src> <dst>
    if ! cp -Rc "$1" "$2" 2>/dev/null; then
        rm -rf "$2"
        cp -R "$1" "$2"
    fi
}

mkdir -p "${STAGE}"
# Copy the prebuilt payload (bin/ services/ scripts/ .env.example VERSION).
cp -R "${BUNDLE}/." "${STAGE}/"
# Drop npm-package metadata that isn't part of the app.
rm -f "${STAGE}/package.json" "${STAGE}/README.md" "${STAGE}/.gitignore" "${STAGE}/.npmignore"

# One-time migration: move credentials from the old app/.env location to the
# canonical ~/.meridian/.env (outside the swap area — untouched by updates).
# If both exist, the canonical wins and the old copy is removed.
if [[ -f "${APP}/.env" ]]; then
    if [[ ! -f "${HOME}/.meridian/.env" ]]; then
        mv "${APP}/.env" "${HOME}/.meridian/.env"
        echo "migrated credentials: ~/.meridian/app/.env → ~/.meridian/.env"
    else
        rm -f "${APP}/.env"
    fi
fi

# Preserve the Python venv across updates. The venv is built from PyPI via
# uv sync at install time; preserving it means install-from-bundle.sh only
# re-syncs when uv.lock actually changed.
if [[ -d "${APP}/services/.venv" ]]; then
    mkdir -p "${STAGE}/services"
    rm -rf "${STAGE}/services/.venv"
    clone_dir "${APP}/services/.venv" "${STAGE}/services/.venv"
fi

# The dashboard is no longer staged here: it's embedded in the tray binary, so
# the bundle ships neither ui.tar.gz nor a ui/ dir. Any old ~/.meridian/app/ui
# from a pre-fold install is discarded by the swap below (APP → OLD → rm).

# The swap: two renames on one filesystem. Running daemons keep their open
# inodes from the old tree until they restart; the installer (exec'd next)
# reloads them against the new tree.
if [[ -d "${APP}" ]]; then mv "${APP}" "${OLD}"; fi
mv "${STAGE}" "${APP}"
rm -rf "${OLD}"

exec bash "${APP}/scripts/install-from-bundle.sh" "$@"
