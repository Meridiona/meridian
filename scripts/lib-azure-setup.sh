#!/usr/bin/env bash
# meridian — normalises screenpipe activity into structured app sessions
#
# Shared Azure DevOps setup helpers, sourced by both install.sh (source installs)
# and scripts/install-from-bundle.sh (bundle installs). The sourcing script must
# already define: info, ok, warn, get_env_value, set_env_value.
#
# PAT auth only — Azure DevOps does not expose a standard OAuth 2.0 device-flow.
# Global PATs are being retired by Microsoft; users should create org-scoped or
# project-scoped PATs with Work Items (Read & write) permission.

# Interactive Azure DevOps setup. Prompts for PAT + org/project and writes them
# to the supplied .env file. Skips individual keys that are already set.
setup_azure_devops() {
    local env_file="$1"

    # PAT
    if [[ -n "$(get_env_value AZURE_DEVOPS_PAT "$env_file")" ]]; then
        ok "AZURE_DEVOPS_PAT already set — keeping"
    else
        echo "    Create a Personal Access Token at:"
        echo "      https://dev.azure.com → User settings → Personal access tokens"
        echo "    Required scope: Work Items (Read & write)"
        echo "    Note: project-scoped or org-scoped PATs are preferred over global PATs."
        echo
        local _pat=""
        read -r -s -p "    Azure DevOps PAT: " _pat
        echo
        if [[ -n "$_pat" ]]; then
            set_env_value AZURE_DEVOPS_PAT "$_pat" "$env_file"
            ok "AZURE_DEVOPS_PAT set"
        else
            warn "  No PAT entered — Azure DevOps integration skipped"
            return 0
        fi
    fi

    # Organisation
    if [[ -n "$(get_env_value AZURE_DEVOPS_ORG "$env_file")" ]] || \
       [[ -n "$(get_env_value AZURE_DEVOPS_ORG_URL "$env_file")" ]]; then
        ok "AZURE_DEVOPS_ORG already set — keeping"
    else
        echo
        echo "    Organisation URL formats:"
        echo "      Standard cloud:  enter just the org name  (e.g. mycompany)"
        echo "      Legacy cloud:    enter  mycompany.visualstudio.com"
        echo "      On-premises:     enter the full URL        (e.g. https://tfs.corp/DefaultCollection)"
        echo
        local _org=""
        read -r -p "    Organisation name or URL: " _org
        if [[ -z "$_org" ]]; then
            warn "  No organisation entered — Azure DevOps integration may be incomplete"
        elif [[ "$_org" == *"://"* ]]; then
            set_env_value AZURE_DEVOPS_ORG_URL "$_org" "$env_file"
            ok "AZURE_DEVOPS_ORG_URL set (on-prem / full URL)"
        else
            set_env_value AZURE_DEVOPS_ORG "$_org" "$env_file"
            ok "AZURE_DEVOPS_ORG set"
        fi
    fi

    # Project
    if [[ -n "$(get_env_value AZURE_DEVOPS_PROJECT "$env_file")" ]]; then
        ok "AZURE_DEVOPS_PROJECT already set — keeping"
    else
        local _project=""
        read -r -p "    Project name: " _project
        if [[ -n "$_project" ]]; then
            set_env_value AZURE_DEVOPS_PROJECT "$_project" "$env_file"
            ok "AZURE_DEVOPS_PROJECT set"
        else
            warn "  No project entered — Azure DevOps integration may be incomplete"
        fi
    fi
}
