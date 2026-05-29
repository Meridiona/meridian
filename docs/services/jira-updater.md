# Jira Updater

Fetches in-progress Jira tasks, queries Meridian MCP for session data on each task, generates a bullet-point summary via hermes, and posts timed comments to Jira.

Updates are logged to `jira_update_log` for idempotent deduplication per `(task_key, period_start, period_end)` slot — re-running the same window never double-posts.

Default schedule: fires at 1 PM and 5 PM within office hours (9–17), looking back over the preceding 4-hour window.

## Prerequisites

Set these in `services/.env`:

```bash
JIRA_BASE_URL=https://your-instance.atlassian.net
JIRA_EMAIL=your-email@example.com
JIRA_API_TOKEN=your-api-token
```

Meridian MCP must be built:

```bash
cd packages/meridian-mcp
npm run build
```

## Quick commands

```bash
# One-shot: update all in-progress tasks (uses current 4-hour window)
python -m agents.jira_updater_daemon --trigger-now

# One-shot: update a single task
python -m agents.jira_updater_daemon --task KAN-87

# Dry run: print comments without posting
python -m agents.jira_updater_daemon --dry-run

# Custom look-back window
python -m agents.jira_updater_daemon --interval 2

# Long-running daemon (sleeps until next scheduled slot)
python -m agents.jira_updater_daemon
```

## Daemon lifecycle

```bash
# Install and start
./scripts/install-jira-updater-daemon.sh

# Stop and remove
./scripts/uninstall-jira-updater-daemon.sh

# Status and logs
launchctl print gui/$(id -u)/com.meridiona.jira-updater-daemon
tail -f ~/.meridian/logs/jira-updater.log
```

## Configuration

| Variable | Default | Description |
|---|---|---|
| `UPDATE_INTERVAL_HOURS` | `4` | Hours between scheduled slots |
| `OFFICE_START_HOUR` | `9` | Office start hour (UTC) |
| `OFFICE_END_HOUR` | `17` | Office end hour (UTC) |
| `JIRA_POST_NO_ACTIVITY` | `1` | Post comment even if no sessions found (`0` to skip) |
| `MERIDIAN_MCP_PATH` | Auto-detected | Path to `packages/meridian-mcp/dist/index.js` |
