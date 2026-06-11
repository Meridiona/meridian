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
# Copy the prebuilt payload (bin/ ui.tar.gz services/ scripts/ .env.example VERSION).
cp -R "${BUNDLE}/." "${STAGE}/"
# Drop npm-package metadata that isn't part of the app.
rm -f "${STAGE}/package.json" "${STAGE}/README.md" "${STAGE}/.gitignore" "${STAGE}/.npmignore"

# Preserve an existing .env across re-installs/updates.
[[ -f "${APP}/.env" ]] && cp "${APP}/.env" "${STAGE}/.env"

# Preserve the Python venv across updates. The venv is built from PyPI via
# uv sync at install time; preserving it means install-from-bundle.sh only
# re-syncs when uv.lock actually changed.
if [[ -d "${APP}/services/.venv" ]]; then
    mkdir -p "${STAGE}/services"
    rm -rf "${STAGE}/services/.venv"
    clone_dir "${APP}/services/.venv" "${STAGE}/services/.venv"
fi

# UI. Two cases, mirroring install-from-bundle.sh's contract:
#   * unchanged (tarball hash matches the recorded one, or no tarball shipped):
#     clone the existing ui/ and drop the tarball — the installer reads
#     "no tarball + ui/ present" as unchanged and skips re-extraction.
#   * changed: pre-extract the tarball into staging AND keep the tarball. The
#     stage is then complete BEFORE the swap, so an installer crash after the
#     swap still leaves a runnable dashboard; the installer's own extraction
#     (which records the new hash) re-does only a few seconds of work.
_hash_file="${HOME}/.meridian/.component-hashes"
_ui_preserved=0
if [[ -d "${APP}/ui" ]]; then
    if [[ -f "${STAGE}/ui.tar.gz" ]] && [[ -f "${_hash_file}" ]]; then
        _old_ui_hash="$(grep '^ui_tarball=' "${_hash_file}" 2>/dev/null | cut -d= -f2 || true)"
        _new_ui_hash="$(shasum -a 256 "${STAGE}/ui.tar.gz" | cut -d' ' -f1)"
        if [[ -n "${_old_ui_hash}" && "${_new_ui_hash}" == "${_old_ui_hash}" ]]; then
            clone_dir "${APP}/ui" "${STAGE}/ui"
            rm -f "${STAGE}/ui.tar.gz"
            _ui_preserved=1
        fi
    elif [[ ! -f "${STAGE}/ui.tar.gz" ]]; then
        # No tarball in bundle = UI unchanged since last release; keep existing build.
        clone_dir "${APP}/ui" "${STAGE}/ui"
        _ui_preserved=1
    fi
fi
if [[ "${_ui_preserved}" -eq 0 && -f "${STAGE}/ui.tar.gz" ]]; then
    mkdir -p "${STAGE}/ui"
    tar -xzf "${STAGE}/ui.tar.gz" -C "${STAGE}/ui"
fi

# The swap: two renames on one filesystem. Running daemons keep their open
# inodes from the old tree until they restart; the installer (exec'd next)
# reloads them against the new tree.
if [[ -d "${APP}" ]]; then mv "${APP}" "${OLD}"; fi
mv "${STAGE}" "${APP}"
rm -rf "${OLD}"

exec bash "${APP}/scripts/install-from-bundle.sh" "$@"
