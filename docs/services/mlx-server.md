# MLX Server

A persistent FastAPI process (port 7823) that loads Qwen3.5-9B once at startup. The Rust daemon HTTP-calls it for every session classification. No per-request cold load.

Requires Apple Silicon (Metal).

## Install as a launchd daemon (recommended)

```bash
bash services/scripts/install-mlx-server-daemon.sh [--port 7823]

# Verify the model loaded
tail -f ~/.meridian/logs/mlx-server.log
# Expected: "server: MLX model ready"

# Status / stop / restart
launchctl print gui/$(id -u)/com.meridiona.mlx-server
bash services/scripts/uninstall-mlx-server-daemon.sh
```

## Run manually (development)

```bash
cd services
.venv313/bin/meridian-server --backend mlx --port 7823
# or
python -m agents.server --backend mlx --port 7823
```

## Model

**Qwen3.5-9B-OptiQ-4bit** — downloaded from Hugging Face on first run (~4 GB). Subsequent starts load from local cache in ~5 s.

The Rust daemon TCP-connects to the server at startup to verify it is reachable before entering the poll loop. If the server is not running, the daemon exits immediately with a clear error.

## Configuration

| Variable | Default | Description |
|---|---|---|
| `MLX_SERVER_PORT` | `7823` | Port to listen on |
| `MLX_SERVER_URL` | (unset → in-process load) | Full URL of a running server; used by the eval pipeline |
| `CLASSIFICATION_TIMEOUT_S` | `120` | Per-session inference timeout |

## Disable classification

Set `CLASSIFICATION_ENABLED=false` in `~/.meridian/.env` to skip classification entirely. The daemon runs ETL and activity categorisation only, with no MLX server needed.
