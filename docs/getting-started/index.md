# Installation

## Prerequisites

- macOS 13+ with Apple Silicon (the MLX inference server requires Metal)
- [screenpipe](https://screenpi.pe) running and recording
- Internet connection (for Homebrew + dependency downloads on first install)

`install.sh` handles everything else — Homebrew, Rust 1.93.1, Node 18+, Python 3.13, and screenpipe itself.

## Install

```bash
git clone https://github.com/meridiona/meridian
cd meridian
./install.sh
```

The installer:
1. Detects and installs any missing prerequisites
2. Builds the Rust daemon, MCP server, and Next.js dashboard
3. Sets up the Python services and downloads the MLX model (~4 GB, first run only)
4. Walks you through granting screenpipe its macOS permissions (Screen Recording, Accessibility, Microphone)
5. Registers four launchd LaunchAgents: `com.meridiona.screenpipe`, `com.meridiona.daemon`, `com.meridiona.jira-updater`, and `com.meridiona.ui`

### Installer flags

| Flag | Effect |
|---|---|
| `--no-ui` | Skip the dashboard build |
| `--dry-run` | Preview actions without executing |
| `--no-daemon` | Build only; don't register launchd agents |
| `--skip-permissions` | Skip the macOS permissions walkthrough |
| `--skip-env` | Skip the credential prompts |
| `--mlx` | Use the persistent MLX inference server (Apple Silicon only) |

## Python environment setup

Task classification uses a persistent MLX inference server (Qwen3.5-9B). Set it up once after cloning:

```bash
cd services

# Create a Python 3.13 virtual environment
python3.13 -m venv .venv313

# Install core dependencies + MLX inference extras
.venv313/bin/pip install -e ".[local-llm]"
```

## Run

Once installed:

```bash
meridian start          # bring up all four daemons
meridian status         # check what's running
meridian logs           # tail the Rust daemon log
meridian logs ui        # tail the dashboard log
meridian doctor         # diagnose missing config / services / permissions
meridian stop           # stop all daemons
```

The dashboard runs at **http://localhost:3000**. Remove everything with `meridian uninstall`.

## What gets created

```
~/.meridian/
  meridian.db       — the normalised SQLite database (~10 MB per ~9k frames)
  logs/
    daemon.log
    mlx-server.log
    tagger-daemon.log
    jira-updater.log
```

Query the database directly:

```bash
sqlite3 ~/.meridian/meridian.db \
  "SELECT app_name, ROUND(SUM(duration_s)/60.0,1) as min, COUNT(*) as n
   FROM app_sessions GROUP BY app_name ORDER BY min DESC LIMIT 10;"
```

## Next steps

- [Configure credentials and env vars →](/getting-started/configuration)
- [Understand how the ETL pipeline works →](/architecture/etl-pipeline)
- [Set up the MCP server for AI tools →](/mcp-server)
