# Meridian Tray App

A Tauri-based macOS menu bar application that shows Meridian daemon status, displays today's top app, and enables quick access to worklog drafting.

## Architecture

- **Rust backend** (`src-tauri/`): Polls health, status, and worklog APIs every 60 seconds
- **Web UI** (`src/`): Lightweight HTML/CSS/JavaScript popover
- **Launchd integration**: Registered as `com.meridiona.tray` to auto-start on login

## Development

```bash
# Install dependencies
npm install

# Start dev server with hot reload
npm run tauri dev

# Lint and format (Rust)
cd src-tauri
cargo fmt
cargo clippy -- -D warnings
cargo test
```

## Building for Release

```bash
# Generate icons
bash create-icons.sh

# Install dependencies
npm install

# Build (release binary)
npm run tauri build

# Output: src-tauri/target/release/meridian-tray
```

## Components

### `src-tauri/src/poll.rs`
60-second polling loop that:
- Calls `/api/health` to check daemon + UI status
- Calls `/api/active` to get current session
- Calls `/api/today` to get focus stats
- Calls `/api/worklogs` to count drafted items

### `src-tauri/src/commands.rs`
Tauri commands exposed to the UI:
- `get_status()` — returns current cached state
- `open_dashboard()` — opens browser to http://127.0.0.1:3939
- `open_worklogs()` — opens browser to worklogs view
- `restart_daemon()` — `launchctl kickstart -k` the daemon
- `toggle_daemon()` — pause/resume the daemon via `launchctl stop/start`

### `src-tauri/src/state.rs`
Shared app state:
- `health`: daemon/UI health status
- `active_session`: current app name + elapsed seconds
- `focus_s`, `switch_count`: today's stats
- `drafts_count`: pending worklog count

### `src/app.js`
Event handlers for the UI:
- Listens for `status-update` events from the Rust backend
- Handles menu/button clicks
- Manages UI rendering and local elapsed-time timer

## Installing as a LaunchAgent

```bash
# Production install (auto-starts on login)
bash scripts/install-tray-daemon.sh

# Uninstall
bash scripts/uninstall-tray-daemon.sh

# View logs
tail -f ~/.meridian/logs/tray.log

# Stop/restart manually
launchctl stop com.meridiona.tray
launchctl start com.meridiona.tray
```

## Notifications

The tray app sends macOS notifications for:
- **Daemon offline**: "Meridian went quiet" (after 2 consecutive health check failures)
- **Daemon back online**: "Back online"
- **Toggle actions**: "Paused" / "Resumed" when user clicks the toggle
- **Worklog drafts**: "X drafts waiting on you" when new drafts appear

Notifications appear in macOS Notification Center and the menu bar.

## Troubleshooting

### Tray app doesn't start
```bash
# Check if it's registered
launchctl list com.meridiona.tray

# View logs
tail -f ~/.meridian/logs/tray.log

# Restart manually
launchctl stop com.meridiona.tray
launchctl start com.meridiona.tray
```

### Health status not updating
- The tray polls every 60 seconds
- Check `/api/health` manually: `curl http://127.0.0.1:3939/api/health | jq`
- Verify the UI is running: `curl http://127.0.0.1:3939/`

### Toggle doesn't work
- Check Rust daemon logs: `tail -f ~/.meridian/logs/meridian.log`
- Verify launchctl recognizes the daemon: `launchctl list | grep meridiona`
