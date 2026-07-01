#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
#
# PR gate: a pull request that changes services/** must bump
# services/pyproject.toml above the live runtime version of the PR's TARGET
# channel. Without it, a merge can land a services/ change whose version is <=
# the published channel, and the runtime auto-upgrade equality check then never
# delivers it to those users (or, on main, a stale version could downgrade them).
#
# Channel by PR base branch:
#   base main      -> runtime-latest   (production)
#   base pre-main  -> runtime-staging   (staging)
#   anything else  -> no channel, check skipped
#
# We check the TARGET channel only, not max(staging, production). pre-main
# legitimately trails main's release version under set-version lockstep, so a
# max() rule would make every staging services PR unmergeable. Promotion safety
# is still enforced loudly: a pre-main -> main promotion is a PR whose BASE is
# main, so it is checked against production-live here — a stale promotion version
# fails this gate instead of silently never shipping. The auto-publish gate's
# strict-greater rule is the final fail-safe (a stale version reaching main just
# does not publish).
#
# Inputs (env): GITHUB_BASE_REF (PR target branch), GH_TOKEN, GITHUB_REPOSITORY.
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/runtime-publish-gate.sh
source "${DIR}/runtime-publish-gate.sh"

: "${GITHUB_BASE_REF:?GITHUB_BASE_REF required (PR target branch)}"

case "${GITHUB_BASE_REF}" in
    main)     CHANNEL="runtime-latest" ;;
    pre-main) CHANNEL="runtime-staging" ;;
    *)        echo "✓ base '${GITHUB_BASE_REF}' has no runtime channel — skipping"; exit 0 ;;
esac

LOCAL_V="$(read_local_version)"
fetch_live_version "${CHANNEL}"   # sets FETCH_STATUS + LIVE_VERSION

case "${FETCH_STATUS}" in
    absent)
        echo "✓ ${CHANNEL} has no published runtime yet — version ${LOCAL_V} accepted"
        exit 0 ;;
    error)
        echo "✗ could not read ${CHANNEL} runtime-manifest.json (fetch error) — failing closed" >&2
        exit 1 ;;
esac

if version_gt "${LOCAL_V}" "${LIVE_VERSION}"; then
    echo "✓ services/pyproject.toml version ${LOCAL_V} > ${CHANNEL} live ${LIVE_VERSION}"
    exit 0
fi

cat >&2 <<EOF
✗ services/ changed but services/pyproject.toml version ${LOCAL_V} is not greater
  than the live ${CHANNEL} runtime (${LIVE_VERSION}).

  Bump services/pyproject.toml to a version > ${LIVE_VERSION} so the rebuilt
  runtime ships as an upgrade. The runtime auto-upgrade is an EQUALITY check, so
  a version <= a live channel never reaches users on that channel.
EOF
exit 1
