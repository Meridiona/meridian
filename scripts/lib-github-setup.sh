#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions
#
# Shared GitHub setup helpers, sourced by both install.sh (source installs) and
# scripts/install-from-bundle.sh (bundle installs). The sourcing script must
# already define: info, ok, warn, get_env_value, set_env_value (resolved at
# call time, so definition order across files does not matter).

# Obtain a GitHub token from the gh CLI — no PAT needed. meridian needs two
# scopes: `repo` (post worklog / task-update issue comments) and `read:project`
# (read Projects v2 via GraphQL). gh's default web-login grants repo + read:org
# but not read:project, so add whatever is missing through the same browser flow,
# then write the OAuth token to GITHUB_TOKEN. Returns non-zero if gh is missing,
# unauthenticated, or the scope refresh fails, so the caller can fall back to a
# manual PAT prompt. An existing GITHUB_TOKEN is kept untouched.
_try_gh_token() {
    local env_file="$1"
    [[ -n "$(get_env_value GITHUB_TOKEN "$env_file")" ]] && {
        ok "GITHUB_TOKEN already set — keeping"; return 0
    }
    command -v gh >/dev/null 2>&1 || return 1
    gh auth status >/dev/null 2>&1 || return 1

    # Add any missing scope through gh's browser flow. `project` (write) satisfies
    # the read:project requirement too, so accept either.
    local status; status="$(gh auth status 2>&1)"
    local want=()
    grep -q "'repo'" <<< "$status" || want+=("repo")
    grep -qE "'read:project'|'project'" <<< "$status" || want+=("read:project")
    if (( ${#want[@]} > 0 )); then
        local joined; printf -v joined '%s,' "${want[@]}"; joined="${joined%,}"
        info "  Granting the ${joined} scope(s) to your gh login (opens a browser)…"
        gh auth refresh -h github.com -s "$joined" >&2 || {
            warn "  Could not extend gh scopes — use a personal access token instead"
            return 1
        }
    fi

    local token
    token="$(gh auth token 2>/dev/null)" || return 1
    [[ -z "$token" ]] && return 1
    set_env_value GITHUB_TOKEN "$token" "$env_file"
    ok "GITHUB_TOKEN set from gh CLI (no PAT needed)"
}

# Interactively pick GitHub Projects and write their node IDs to GITHUB_PROJECT_IDS.
# Lists both personal and org projects via GraphQL. No-op if already set or if
# the gh CLI is unavailable or unauthenticated.
_pick_github_projects() {
    local env_file="$1"
    [[ -n "$(get_env_value GITHUB_PROJECT_IDS "$env_file")" ]] && {
        ok "GITHUB_PROJECT_IDS already set — keeping"; return 0
    }
    command -v gh >/dev/null 2>&1 || return 0
    gh auth status >/dev/null 2>&1 || return 0

    local raw
    raw="$(gh api graphql -f query='
      { viewer {
          projectsV2(first: 20) { nodes { id title } }
          organizations(first: 20) {
            nodes { login projectsV2(first: 20) { nodes { id title } } }
          }
      } }' 2>/dev/null)" || {
        warn "Could not list GitHub Projects — add GITHUB_PROJECT_IDS to the config manually if needed"
        return 0
    }

    # One python3 pass emits "id<TAB>label" per project (personal + org). python3
    # is always present on macOS.
    local pairs_raw
    pairs_raw="$(printf '%s' "$raw" | python3 -c "
import json, sys
d = json.load(sys.stdin).get('data', {}).get('viewer', {})
for n in d.get('projectsV2', {}).get('nodes', []):
    print('%s\t%s' % (n['id'], n['title']))
for org in d.get('organizations', {}).get('nodes', []):
    for n in org.get('projectsV2', {}).get('nodes', []):
        print('%s\t%s / %s' % (n['id'], org['login'], n['title']))
" 2>/dev/null)" || true

    # Split each "id<TAB>label" line into parallel arrays (bash 3.2 — no mapfile).
    local _ids=() _labels=()
    local _id _label
    while IFS=$'\t' read -r _id _label; do
        [[ -z "$_id" ]] && continue
        _ids+=("$_id"); _labels+=("$_label")
    done <<< "$pairs_raw"
    local count=${#_ids[@]}
    (( count == 0 )) && { warn "No GitHub Projects found for your account"; return 0; }

    echo >&2
    echo "    Your GitHub Projects:" >&2
    local i=0
    while (( i < count )); do
        printf "      %d. %s\n" "$((i+1))" "${_labels[$i]}" >&2
        i=$((i+1))
    done
    echo >&2

    local selection
    read -r -p "    Enter project numbers (comma-sep, e.g. 1,2) or Enter to skip: " selection
    [[ -z "$selection" ]] && { info "  (skipped GITHUB_PROJECT_IDS)"; return 0; }

    local selected_ids=()
    local IFS_save="$IFS"
    IFS=',' read -ra nums <<< "$selection"
    IFS="$IFS_save"
    local n
    for n in "${nums[@]}"; do
        n="${n//[[:space:]]/}"
        if [[ "$n" =~ ^[0-9]+$ ]] && (( n >= 1 && n <= count )); then
            selected_ids+=("${_ids[$((n-1))]}")
        fi
    done

    if [[ ${#selected_ids[@]} -eq 0 ]]; then
        info "  (no valid selection — skipped GITHUB_PROJECT_IDS)"; return 0
    fi

    local joined
    printf -v joined '%s,' "${selected_ids[@]}"
    set_env_value GITHUB_PROJECT_IDS "${joined%,}" "$env_file"
    ok "GITHUB_PROJECT_IDS set (${#selected_ids[@]} project(s))"
}
