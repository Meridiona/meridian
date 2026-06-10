#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions
#
# Shared Azure DevOps setup helpers, sourced by both install.sh (source installs)
# and scripts/install-from-bundle.sh (bundle installs). The sourcing script must
# already define: info, ok, warn, get_env_value, set_env_value.

# Interactive Azure DevOps setup.
# Requires just two things from the user: the project URL and a PAT.
setup_azure_devops() {
    local env_file="$1"

    # Skip if already fully configured.
    if [[ -n "$(get_env_value AZURE_DEVOPS_URL "$env_file")" ]] && \
       [[ -n "$(get_env_value AZURE_DEVOPS_PAT "$env_file")" ]]; then
        ok "Azure DevOps already configured — keeping"
        return 0
    fi

    # ── Step 1: Project URL ───────────────────────────────────────────────────
    if [[ -n "$(get_env_value AZURE_DEVOPS_URL "$env_file")" ]] || \
       [[ -n "$(get_env_value AZURE_DEVOPS_ORG "$env_file")" ]] || \
       [[ -n "$(get_env_value AZURE_DEVOPS_ORG_URL "$env_file")" ]]; then
        ok "Azure DevOps URL already set — keeping"
    else
        echo
        echo "    Step 1 of 2 — Project URL"
        echo "    ─────────────────────────────────────────────────────────────"
        echo "    Open your Azure DevOps project in a browser and copy the URL"
        echo "    from the address bar. It will look like one of these:"
        echo
        echo "      Cloud (standard):  https://dev.azure.com/mycompany/MyProject"
        echo "      Cloud (legacy):    https://mycompany.visualstudio.com/MyProject"
        echo "      On-premises:       https://tfs.corp.com/DefaultCollection/MyProject"
        echo
        local _url=""
        read -r -p "    Paste your project URL: " _url
        if [[ -z "$_url" ]]; then
            warn "  No URL entered — Azure DevOps integration skipped"
            return 0
        fi
        set_env_value AZURE_DEVOPS_URL "$_url" "$env_file"
        ok "AZURE_DEVOPS_URL set"
    fi

    # ── Step 2: Personal Access Token ────────────────────────────────────────
    if [[ -n "$(get_env_value AZURE_DEVOPS_PAT "$env_file")" ]]; then
        ok "AZURE_DEVOPS_PAT already set — keeping"
    else
        echo
        echo "    Step 2 of 2 — Personal Access Token (PAT)"
        echo "    ─────────────────────────────────────────────────────────────"
        echo "    In Azure DevOps, go to:"
        echo "      User settings (top-right avatar) → Personal access tokens → New token"
        echo
        echo "    Required scope:  Work Items → Read & write"
        echo "    Tip: choose an org-scoped (not global) PAT — global PATs are"
        echo "    being retired by Microsoft."
        echo
        local _pat=""
        read -r -s -p "    Paste your PAT (hidden): " _pat
        echo
        if [[ -z "$_pat" ]]; then
            warn "  No PAT entered — Azure DevOps integration may be incomplete"
            return 0
        fi
        set_env_value AZURE_DEVOPS_PAT "$_pat" "$env_file"
        ok "AZURE_DEVOPS_PAT set"
    fi
}
