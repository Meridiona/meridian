# meridian — normalises screenpipe activity into structured app sessions
#
# Shared Jira credential-collection helper for install.sh + install-from-bundle.sh.
# OAuth-first: when a runnable meridian binary is available, connect Jira in the
# browser now (writes ~/.meridian/oauth/jira.json — no .env creds, auto-refreshed,
# and the daemon picks it up on its next start/restart with no extra command). The
# static API token is the FALLBACK for users whose Atlassian org blocks
# third-party OAuth apps.
#
# Depends (late-binding) on info/ok/warn + prompt_env_var/get_env_value/set_env_value
# from the sourcing installer. When the meridian binary isn't built yet (source
# install, prompt runs before `cargo build`), this sets the global
# MERIDIAN_JIRA_OAUTH_PENDING=1 so the caller can run `meridian oauth-login jira`
# after the build, before the daemon starts.

# _jira_token_fallback <env_file> — the legacy basic-auth path (email + API token).
_jira_token_fallback() {
    local env_file="$1"
    info "API-token path — create one at https://id.atlassian.com/manage-profile/security/api-tokens"
    prompt_env_var "JIRA_BASE_URL" "Jira URL (e.g. https://your-org.atlassian.net)" 0 "$env_file"
    # The Python side reads JIRA_URL, the Rust side JIRA_BASE_URL — keep both in sync.
    local jira_url
    jira_url="$(get_env_value JIRA_BASE_URL "$env_file")"
    [[ -n "$jira_url" ]] && set_env_value JIRA_URL "$jira_url" "$env_file"
    prompt_env_var "JIRA_EMAIL" "Jira email" 0 "$env_file"
    prompt_env_var "JIRA_API_TOKEN" "Jira API token" 1 "$env_file"
}

# _connect_jira <env_file> <meridian_bin> — OAuth-first Jira connection.
# Offers browser OAuth (recommended); falls back to the API token on decline,
# failure, or when no binary is available yet (then deferred via the global).
_connect_jira() {
    local env_file="$1" bin="${2:-}"
    local ans

    info "Jira sign-in opens your browser — you'll log in to your own Jira site."
    info "  (If your org requires admin approval for third-party apps, choose n and use an API token instead.)"
    read -r -p "  Connect Jira in your browser? (recommended — no API token) [Y/n] " ans
    if [[ "$ans" =~ ^[Nn] ]]; then
        _jira_token_fallback "$env_file"
        prompt_env_var "JIRA_PROJECT_KEYS" "Jira project keys (optional, comma-sep, e.g. KAN,ENG)" 0 "$env_file"
        return 0
    fi

    if [[ -n "$bin" && -x "$bin" ]]; then
        # Binary is ready — open the browser now. The token store the login writes
        # is read by the daemon on its next start/restart (which the installer does
        # after this), so there's no separate command to run.
        info "Opening your browser to authorize Jira…"
        if "$bin" oauth-login jira; then
            ok "Jira connected via browser OAuth"
        else
            warn "Browser sign-in didn't complete."
            warn "If your Atlassian org blocks third-party apps, use an API token instead:"
            _jira_token_fallback "$env_file"
        fi
    else
        # Binary not built yet (source install) — defer the login to after the build.
        MERIDIAN_JIRA_OAUTH_PENDING=1
        info "Will open your browser to authorize Jira once the build finishes."
    fi

    prompt_env_var "JIRA_PROJECT_KEYS" "Jira project keys (optional, comma-sep, e.g. KAN,ENG)" 0 "$env_file"
}

# _run_pending_jira_oauth <meridian_bin> <env_file> — invoked by the source
# installer after the build (binary now exists) to fulfil a deferred OAuth login.
# Falls back to an API-token prompt if the browser flow doesn't complete.
_run_pending_jira_oauth() {
    local bin="$1" env_file="$2"
    [[ "${MERIDIAN_JIRA_OAUTH_PENDING:-0}" == "1" ]] || return 0
    MERIDIAN_JIRA_OAUTH_PENDING=0
    if [[ -x "$bin" ]] && "$bin" oauth-login jira; then
        ok "Jira connected via browser OAuth"
    else
        warn "Jira browser sign-in didn't complete."
        warn "If your org blocks third-party apps, add an API token: meridian config edit"
        _jira_token_fallback "$env_file"
    fi
}
