# meridian — normalises screenpipe activity into structured app sessions
#
# Shared Trello credential-collection helper for install-from-bundle.sh.
# Trello uses a legacy token grant delivered via URL fragment — the daemon
# handles the relay; users just click Accept in the browser.
#
# Depends (late-binding) on info/ok/warn from the sourcing installer.

# _connect_trello <env_file> <meridian_bin> — Browser token flow.
# On bundle installs the binary is always ready; when it isn't, prints a
# manual command the user can run after the build completes.
_connect_trello() {
    local env_file="$1" bin="${2:-}"

    info "Trello sign-in opens your browser — you'll grant Meridian read/write access."
    read -r -p "  Connect Trello in your browser? [Y/n] " ans
    if [[ "$ans" =~ ^[Nn] ]]; then
        info "Skipped. To connect later: meridian oauth-login trello"
        return 0
    fi

    if [[ -n "$bin" && -x "$bin" ]]; then
        info "Opening your browser to authorize Trello…"
        if "$bin" oauth-login trello; then
            ok "Trello connected via browser OAuth"
        else
            warn "Browser sign-in didn't complete."
            warn "To connect later: meridian oauth-login trello"
        fi
    else
        info "Run this once the install finishes:"
        info "  meridian oauth-login trello"
    fi
}
