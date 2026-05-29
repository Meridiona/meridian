# Configuration

All settings are via environment variables. `install.sh` collects them interactively on first run.

## Credential files

Credentials are split across three files, one per daemon:

| File | Used by |
|---|---|
| `~/.meridian/.env` | Rust daemon (Jira, GitHub, Linear, observability) |
| `services/.env` | Python agents (LLM endpoint, Jira, observability) |
| `services/.hermes/.env` | hermes-agent library (`OPENROUTER_API_KEY`) |

To edit credentials after install:

```bash
meridian config edit            # opens ~/.meridian/.env in $EDITOR
$EDITOR services/.env
$EDITOR services/.hermes/.env
```

To re-run only the credential walkthrough (skipping builds):

```bash
./install.sh --skip-permissions
```

## Minimum required variables

```bash
# Jira (required for task classification and PM sync)
JIRA_BASE_URL=https://your-instance.atlassian.net
JIRA_EMAIL=you@example.com
JIRA_API_TOKEN=your-api-token

# Enable task classification
CLASSIFICATION_ENABLED=true
```

Set `CLASSIFICATION_ENABLED=false` to skip classification — the daemon runs ETL and activity categorisation only, with no MLX server needed.

## Environment variable reference

### Rust daemon

| Variable | Default | Description |
|---|---|---|
| `SCREENPIPE_DB` | `~/.screenpipe/db.sqlite` | Path to screenpipe's database (read-only) |
| `MERIDIAN_DB` | `~/.meridian/meridian.db` | Path where Meridian writes its database |
| `POLL_INTERVAL_SECS` | `60` | How often to check for new screenpipe frames |
| `CLASSIFICATION_ENABLED` | `true` | Enable session→task classification via MLX server |
| `MLX_SERVER_PORT` | `7823` | Port the persistent MLX inference server listens on |
| `CLASSIFIER_BACKEND` | `mlx` | Classification backend (`mlx` is the only supported value) |
| `CLASSIFICATION_TIMEOUT_S` | `120` | Per-session inference timeout in seconds |
| `RUST_LOG` | `meridian=info` | Tracing filter |
| `MERIDIAN_OTLP_ENDPOINT` | (unset) | OpenObserve OTLP/HTTP traces endpoint |
| `MERIDIAN_OO_AUTH` | (unset) | Base64 `user:password` for OpenObserve OTLP auth |

Tilde expansion is handled automatically. Never hardcode paths.

### Python agents

| Variable | Default | Description |
|---|---|---|
| `MERIDIAN_DB` | `~/.meridian/meridian.db` | Path to the SQLite file (must already exist) |
| `MLX_SERVER_PORT` | `7823` | Port the MLX inference server listens on |
| `CLASSIFICATION_ENABLED` | `true` | Set to `false` to disable classification |
| `SESSION_TEXT_CAP` | `2500` | Per-session OCR/a11y excerpt cap in chars for the classifier prompt |

### Jira updater

| Variable | Default | Description |
|---|---|---|
| `UPDATE_INTERVAL_HOURS` | `4` | Hours between scheduled update slots |
| `OFFICE_START_HOUR` | `9` | Office start hour (UTC) |
| `OFFICE_END_HOUR` | `17` | Office end hour (UTC) |
| `JIRA_POST_NO_ACTIVITY` | `1` | Post comment even if no sessions found in the slot |
| `JIRA_BASE_URL` | — | Jira Cloud instance URL |
| `JIRA_EMAIL` | — | Email address for Jira API token auth |
| `JIRA_API_TOKEN` | — | API token for Jira REST API |

## Example: override poll interval

```bash
POLL_INTERVAL_SECS=30 ./target/release/meridian
```
