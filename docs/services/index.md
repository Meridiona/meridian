# Python Agent

The Python service runs alongside the Rust daemon. It classifies completed `app_sessions` rows to Jira tasks using a persistent MLX inference server, and posts timed progress comments back to Jira.

All writes to `meridian.db` from this service are idempotent — both `ticket_links` and `session_dimensions` use `ON CONFLICT DO UPDATE`.

## Installation

Requires Python 3.13 and Apple Silicon.

```bash
cd services

# Create a Python 3.13 virtual environment
python3.13 -m venv .venv313

# Install core + MLX inference dependencies
.venv313/bin/pip install -e ".[local-llm]"
```

## Task classifier

The Rust daemon calls `POST /classify_sessions` on the MLX server on each intelligence tick. The classifier receives session text (OCR, window titles, accessibility elements) and returns a `task_key`, `session_type`, and `confidence`.

To inspect or re-run classification manually:

```bash
# Re-run classification without writing to DB
python -m agents.tagger --session <id> --dry-run

# Re-run and persist (resets dims + ticket_link first)
python -m agents.tagger --session <id>

# Read-only view of what's stored
python -m agents.tagger --show <id>
```

### Daemon lifecycle

```bash
# Install and start as a launchd agent
./scripts/install-tagger-daemon.sh

# Stop and remove
./scripts/uninstall-tagger-daemon.sh

# Status and logs
launchctl print gui/$(id -u)/com.meridiona.tagger-daemon
tail -f ~/.meridian/logs/tagger-daemon.log
```

## hermes dev mode

By default the pipeline imports from the installed `hermes-agent` package. To step into hermes internals:

```bash
# Clone hermes source into services/.hermes/ (gitignored)
git clone --branch v2026.4.30 https://github.com/NousResearch/hermes-agent.git services/.hermes

# Enable dev mode
echo "HERMES_DEV_MODE=1" >> services/.env
```

Set `HERMES_DEV_MODE=0` to revert to the installed package.

## See also

- [MLX Server →](/services/mlx-server) — persistent inference server
- [Jira Updater →](/services/jira-updater) — automated ticket comments
