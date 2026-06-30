#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
#
# Runtime-publish gate — the single source of truth for the auto-publish flow
# (build-mlx-runtime.yml). Decides WHETHER a CI event should publish a new MLX
# runtime and to WHICH channel, so the "version-skip is an equality check" rule
# lives in CI instead of a human's head.
#
# Channel by ref:
#   branch main             -> runtime-latest   (production; environment-gated)
#   branch pre-main         -> runtime-staging   (staging; auto)
#   tag    runtime-v*       -> runtime-latest    (manual escape hatch)
#   tag    runtime-staging-v* -> runtime-staging  (manual escape hatch)
#   workflow_dispatch / anything else -> "" (build-only, never publishes)
#   NOTE: the app's own semantic-release tags (v*) deliberately map to "" so an
#   app release never republishes the runtime.
#
# Publish decision is FAIL-CLOSED. Publish only when the local
# services/pyproject.toml version DIFFERS from the channel's live
# runtime-manifest.json version:
#   - channel release absent      -> publish (first publish on that channel)
#   - manifest fetch/parse error  -> DO NOT publish (a transient gh failure must
#                                    never read as "version differs -> ship")
#   - versions equal              -> skip (clients would equality-skip anyway)
#   - versions differ             -> publish
#
# Usage:
#   runtime-publish-gate.sh             # CI mode: reads GITHUB_* env, writes $GITHUB_OUTPUT
#   runtime-publish-gate.sh --self-test # run the decision-table unit tests
set -euo pipefail

# ---------------------------------------------------------------------------
# Pure decision helpers (no I/O — unit-tested by --self-test)
# ---------------------------------------------------------------------------

# version_gt <a> <b> -> exit 0 iff a > b, semver-ish via `sort -V`.
version_gt() {
    [[ "$1" != "$2" ]] && [[ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | tail -n1)" == "$1" ]]
}

# decide_publish <local_version> <fetch_status> <live_version> <mode>
#   fetch_status: ok | absent | error
#   mode:
#     auto   (branch push)  -> publish only when local > live. Never downgrades
#            and never republishes an equal version — the branch version number
#            is not a hand-maintained counter (set-version stamps it at release),
#            so a merge that lands a STALE version must fail to ship, not ship.
#     manual (tag push)     -> publish on any difference. Trusts the human; this
#            is the rollback / re-pin escape hatch (e.g. ship an older runtime to
#            undo a bad one).
#   echoes "true" | "false"
decide_publish() {
    local local_v="$1" status="$2" live_v="${3:-}" mode="${4:-auto}"
    case "${status}" in
        error)  echo "false"; return ;;                                      # fail-closed
        absent) echo "true";  return ;;                                      # first publish
        ok)     ;;                                                           # compare below
        *)      echo "false"; return ;;                                      # unknown -> fail-closed
    esac
    if [[ "${mode}" == "manual" ]]; then
        if [[ "${local_v}" != "${live_v}" ]]; then echo "true"; else echo "false"; fi
    else
        if version_gt "${local_v}" "${live_v}"; then echo "true"; else echo "false"; fi
    fi
}

# channel_for_ref <event_name> <ref_type> <ref_name>
#   echoes "runtime-latest" | "runtime-staging" | "" (no publish)
channel_for_ref() {
    local event="$1" reftype="$2" refname="$3"
    [[ "${event}" == "workflow_dispatch" ]] && { echo ""; return; }
    case "${reftype}" in
        tag)
            case "${refname}" in
                runtime-staging-v*) echo "runtime-staging" ;;
                runtime-v*)         echo "runtime-latest" ;;
                *)                  echo "" ;;                               # app v* tags etc. never publish runtime
            esac ;;
        branch)
            case "${refname}" in
                main)     echo "runtime-latest" ;;
                pre-main) echo "runtime-staging" ;;
                *)        echo "" ;;
            esac ;;
        *) echo "" ;;
    esac
}

# ---------------------------------------------------------------------------
# I/O helpers (CI mode)
# ---------------------------------------------------------------------------

read_local_version() {
    grep -m1 '^version' services/pyproject.toml | sed -E 's/.*"([^"]+)".*/\1/'
}

# fetch_live_version <channel> -> sets FETCH_STATUS (ok|absent|error) + LIVE_VERSION.
# absent = the channel rolling release does not exist yet; error = it exists but
# the manifest could not be fetched/parsed (treated as a hard stop upstream).
fetch_live_version() {
    local channel="$1" tmp out rc
    out="$(gh api "repos/${GITHUB_REPOSITORY}/releases/tags/${channel}" 2>&1)" && rc=0 || rc=$?
    if [[ ${rc} -ne 0 ]]; then
        if grep -qi 'HTTP 404\|Not Found' <<<"${out}"; then
            FETCH_STATUS="absent"; LIVE_VERSION=""
        else
            FETCH_STATUS="error"; LIVE_VERSION=""
        fi
        return
    fi
    tmp="$(mktemp -d)"
    if ! gh release download "${channel}" --repo "${GITHUB_REPOSITORY}" \
            -p runtime-manifest.json -D "${tmp}" --clobber >/dev/null 2>&1; then
        FETCH_STATUS="error"; LIVE_VERSION=""; rm -rf "${tmp}"; return
    fi
    LIVE_VERSION="$(python3 -c "import json; print(json.load(open('${tmp}/runtime-manifest.json'))['version'])" 2>/dev/null || echo "")"
    rm -rf "${tmp}"
    if [[ -z "${LIVE_VERSION}" ]]; then FETCH_STATUS="error"; else FETCH_STATUS="ok"; fi
}

# ---------------------------------------------------------------------------
# Self-test — exercises the decision table the advisor called out.
# ---------------------------------------------------------------------------

self_test() {
    local fails=0
    assert() { # <description> <expected> <actual>
        if [[ "$2" == "$3" ]]; then
            echo "  ✓ $1"
        else
            echo "  ✗ $1 — expected '$2', got '$3'" >&2; fails=$((fails + 1))
        fi
    }

    echo "decide_publish (auto / branch — strict >):"
    assert "newer -> publish"          "true"  "$(decide_publish 1.67.0 ok 1.66.2 auto)"
    assert "older -> skip (no downgrade)" "false" "$(decide_publish 1.64.1 ok 1.66.2 auto)"
    assert "equal -> skip"             "false" "$(decide_publish 1.67.0 ok 1.67.0 auto)"
    assert "absent -> first publish"   "true"  "$(decide_publish 1.67.0 absent '' auto)"
    assert "fetch error -> fail-closed" "false" "$(decide_publish 1.67.0 error '' auto)"

    echo "decide_publish (manual / tag — any difference):"
    assert "newer -> publish"          "true"  "$(decide_publish 1.67.0 ok 1.66.2 manual)"
    assert "older -> publish (rollback)" "true"  "$(decide_publish 1.64.1 ok 1.66.2 manual)"
    assert "equal -> skip"             "false" "$(decide_publish 1.67.0 ok 1.67.0 manual)"
    assert "fetch error -> fail-closed" "false" "$(decide_publish 1.67.0 error '' manual)"

    echo "channel_for_ref:"
    assert "dispatch -> none"          ""                "$(channel_for_ref workflow_dispatch branch main)"
    assert "tag runtime-staging-v*"    "runtime-staging" "$(channel_for_ref push tag runtime-staging-v1.64.1)"
    assert "tag runtime-v*"            "runtime-latest"  "$(channel_for_ref push tag runtime-v1.64.1)"
    assert "app tag v* -> none"        ""                "$(channel_for_ref push tag v1.67.0)"
    assert "branch main -> production" "runtime-latest"  "$(channel_for_ref push branch main)"
    assert "branch pre-main -> staging" "runtime-staging" "$(channel_for_ref push branch pre-main)"
    assert "feature branch -> none"    ""                "$(channel_for_ref push branch feat/x)"

    if [[ ${fails} -eq 0 ]]; then
        echo "✓ runtime-publish-gate self-test passed"
    else
        echo "✗ ${fails} self-test assertion(s) failed" >&2; exit 1
    fi
}

# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------

# Sourcing this file exposes the helpers (version_gt, decide_publish,
# read_local_version, fetch_live_version) without running the CLI — used by
# check-runtime-version-bump.sh so the version logic lives in one place.
if [[ "${BASH_SOURCE[0]}" != "${0}" ]]; then
    return 0 2>/dev/null || true
fi

if [[ "${1:-}" == "--self-test" ]]; then
    self_test
    exit 0
fi

EVENT="${GITHUB_EVENT_NAME:?GITHUB_EVENT_NAME required}"
REFTYPE="${GITHUB_REF_TYPE:?GITHUB_REF_TYPE required}"
REFNAME="${GITHUB_REF_NAME:?GITHUB_REF_NAME required}"

# Tag pushes are a human's deliberate act (rollback/re-pin allowed); branch
# pushes are automated and must never downgrade.
MODE="auto"
[[ "${REFTYPE}" == "tag" ]] && MODE="manual"

CHANNEL="$(channel_for_ref "${EVENT}" "${REFTYPE}" "${REFNAME}")"
VERSION=""
SHOULD_PUBLISH="false"
FETCH_STATUS="n/a"
LIVE_VERSION=""

if [[ -n "${CHANNEL}" ]]; then
    VERSION="$(read_local_version)"
    fetch_live_version "${CHANNEL}"
    SHOULD_PUBLISH="$(decide_publish "${VERSION}" "${FETCH_STATUS}" "${LIVE_VERSION}" "${MODE}")"
fi

echo "gate: event=${EVENT} ref=${REFTYPE}/${REFNAME} mode=${MODE} channel=${CHANNEL:-<none>} local=${VERSION:-<none>} live=${LIVE_VERSION:-<none>} fetch=${FETCH_STATUS} -> should_publish=${SHOULD_PUBLISH}" >&2

{
    echo "channel=${CHANNEL}"
    echo "version=${VERSION}"
    echo "should_publish=${SHOULD_PUBLISH}"
    echo "fetch_status=${FETCH_STATUS}"
    echo "live_version=${LIVE_VERSION}"
} >> "${GITHUB_OUTPUT:-/dev/stdout}"
