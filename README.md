# Meridian

Meridian is a lightweight Rust daemon that reads [screenpipe](https://screenpi.pe)'s
ambient recording database and normalises raw frames into structured, app-based activity
sessions stored in its own SQLite database.

It runs continuously on the developer's machine alongside screenpipe, consuming zero
network and minimal CPU/RAM.

## What it produces

Every time the focused app changes, Meridian closes the previous session and writes a row
to `app_sessions`:

| Column | Description |
|---|---|
| `app_name` | App that owned the session |
| `started_at` / `ended_at` | ISO8601 UTC timestamps |
| `duration_s` | Wall-clock seconds |
| `frame_count` | Number of screenpipe frames in the block |
| `category` | AI-assigned activity category (e.g. `coding`, `meeting`, `research`) |
| `confidence` | Category confidence score (0.0–1.0) |
| `window_titles` | JSON array of `{title, count}` — top windows seen |
| `ocr_samples` | JSON array of up to 20 deduplicated OCR text samples |
| `elements_samples` | JSON array of up to 20 deduplicated accessibility tree samples |
| `audio_snippets` | JSON array of transcribed audio (stored in DB; not sent to LLMs) |
| `signals` | JSON array of deduplicated clipboard copies and app-switch events |
| `min_frame_id` / `max_frame_id` | Frame-id range linking back to screenpipe |

## Prerequisites

- macOS with Apple Silicon (the MLX inference server requires Metal)
- [screenpipe](https://screenpi.pe) running and recording
- Rust 1.93.1 — install via `rustup` or `rust-toolchain.toml` is picked up automatically
- Python 3.11 — install via `brew install python@3.11` or `pyenv install 3.11` (outlines/MLX require ≤ 3.13; 3.11 is the supported floor)

## Getting started

**Requirements:** macOS on **Apple Silicon** (M1+). That's it — the installer brings the rest.

### Install (npm)

```bash
npm install -g @meridiona/meridian
meridian setup
```

No clone, no build. The package ships a prebuilt binary; `meridian setup` copies the app to `~/.meridian/app`, installs any missing prerequisites (Homebrew packages, Node, Python 3.11, screenpipe, ffmpeg), sets up the on-device model, and registers four auto-starting services — **screenpipe** (capture), the **daemon** (pipeline), the **MLX server** (on-device model), and the **dashboard** at http://localhost:3939.

`meridian setup` then walks you through screenpipe's two macOS permissions (Screen Recording, Accessibility — audio is disabled, so no Microphone). Connect Jira afterwards (optional).

👉 **Full walkthrough — permissions, Jira, verifying, logs, updating, troubleshooting: [SETUP.md](SETUP.md).**

```bash
meridian status      # the four services
meridian logs -f     # watch the pipeline
meridian config edit # add your Jira credentials
meridian update      # update to the latest release
```

### Build from source (contributors)

```bash
git clone https://github.com/Meridiona/meridian
cd meridian
./install.sh
```

Builds the daemon + UI from source and registers the same services. Flags: `--no-ui`, `--dry-run`, `--no-daemon`, `--skip-permissions`, `--skip-env`, `--mlx-port N` (MLX server port, default 7823).

Once installed, use `meridian dev …` to rebuild + restart after edits — see [Development](#development).

### Configure

`./install.sh` walks you through credential prompts grouped by category:

- **Cloud LLM** — `OPENROUTER_API_KEY` (skip if you're running a local LLM)
- **Jira** — URL, email, API token, project keys (gated by `[y/N]`)
- **GitHub** — token (auto-extracted from the `gh` CLI, no PAT needed) + GitHub Projects to sync
- **Linear** — API key, team IDs
- **Observability (OpenObserve)** — base64 auth + OTLP endpoints

Empty input skips that variable. Values already in `.env` are preserved on re-run.

All credentials are written to a **single repo-root `.env`** — the one config file shared by the Rust daemon and the Python services. Nothing is read from outside the repo (the database and logs still live under `~/.meridian/`). The UI keeps its own `ui/.env.local` (Next.js convention; never put backend secrets there).

Minimum required variables:

```bash
# Jira (for task classification and the PM-worklog stage)
JIRA_BASE_URL=https://your-instance.atlassian.net
JIRA_EMAIL=you@example.com
JIRA_API_TOKEN=your-api-token

# Enable task classification
CLASSIFICATION_ENABLED=true
```

> Set `CLASSIFICATION_ENABLED=false` to skip classification — the daemon runs ETL and FM categorisation only, with no MLX server needed.

To edit credentials later:

```bash
meridian config edit            # opens the repo-root .env in $EDITOR
```

To re-run only the credential walkthrough (skipping builds/permissions):

```bash
# Re-prompt: delete the value(s) you want to re-set, then re-run install.sh.
# Note: install.sh skips any variable already populated.
./install.sh --skip-permissions
```

If you want the prompts off entirely:

```bash
./install.sh --skip-env
```

### The MLX inference server

`./install.sh` installs and starts the MLX server (`com.meridiona.mlx-server`) for you — it powers both session classification and the PM-worklog synthesiser, and the Rust daemon TCP-connects to it on startup. You normally don't touch it directly. For dev/debugging you can run it manually:

```bash
cd services
.venv/bin/meridian-server --backend mlx --port 7823
```

The server loads the model once at startup (~30 s on first run while the model downloads, ~5 s from cache). You will see `server: MLX model ready` in `~/.meridian/logs/mlx-server.log` when it is ready.

### Run

```bash
meridian start          # bring up all daemons (screenpipe + daemon + mlx-server + ui)
meridian status         # check what's running
meridian worklog-status # today's PM worklogs: done/pending + per-ticket comments
meridian doctor         # diagnose missing config / services / permissions
meridian permissions    # re-run the screenpipe permissions walkthrough
```

`meridian worklog-status [--day YYYY-MM-DD]` prints a no-SQL summary of the PM-worklog stage: hours done/pending/stuck, worklogs by state (drafted/approved/posted/skipped/failed), and a per-ticket table with the synthesised Jira comment — plus a "flagged" list of low-confidence or risk-flagged worklogs to inspect.

**Nothing posts to Jira automatically.** The daemon only *drafts* worklogs; you review, edit, and approve each one in the dashboard's **Worklogs** view, and the daemon posts approved worklogs to Jira within ~60s (or run `meridian worklog-post-approved` to post them now). Approval is the only gate.

### Logs

`meridian logs [target] [-f]` tails a log; add `-f` to stream live. Each component has a normal log (everything) and an `-error` log (WARN/ERROR only):

```bash
meridian logs -f                  # Rust daemon — the whole pipeline (default)
meridian logs daemon-error -f     # Rust daemon — problems only
meridian logs mlx-server -f       # MLX inference server — classify/synth requests
meridian logs mlx-server-error -f # MLX server — problems only
meridian logs ui -f               # dashboard
meridian logs screenpipe -f       # capture layer
```

Targets: `daemon` · `daemon-error` · `mlx-server` · `mlx-server-error` · `ui` · `ui-error` · `screenpipe` · `screenpipe-error`. The Rust daemon also writes structured JSON to `~/.meridian/logs/meridian-rust.jsonl.<date>` for grepping.

Once started, the dashboard is at **http://localhost:3939** (set `MERIDIAN_UI_PORT` in `~/.meridian/app/.env` to change it). Stop with `meridian stop`. Remove everything with `meridian uninstall`.

On startup the daemon TCP-connects to the MLX server to verify it is reachable before entering the poll loop. If the server is not running it exits immediately with a clear error message. Stop the daemon with `Ctrl-C` or `SIGTERM`.

## Configuration

These variables are collected interactively by `./install.sh`. The table below is the authoritative reference for what each one means.

All settings are via environment variables; defaults work out of the box.

| Variable | Default | Description |
|---|---|---|
| `SCREENPIPE_DB` | `~/.screenpipe/db.sqlite` | Path to screenpipe's database (read-only) |
| `MERIDIAN_DB` | `~/.meridian/meridian.db` | Path where Meridian writes its database |
| `POLL_INTERVAL_SECS` | `60` | How often to check for new screenpipe frames |
| `CLASSIFICATION_ENABLED` | `true` | Enable session→task classification via MLX server |
| `MLX_SERVER_PORT` | `7823` | Port the persistent MLX inference server listens on |
| `CLASSIFICATION_TIMEOUT_S` | `120` | Per-session inference timeout in seconds |

Example:
```bash
POLL_INTERVAL_SECS=30 ./target/release/meridian
```

## Data location

```
~/.meridian/meridian.db       — the normalised database (10 MB per ~9k frames)
```

Query it directly:
```bash
sqlite3 ~/.meridian/meridian.db \
  "SELECT app_name, ROUND(SUM(duration_s)/60.0,1) as min, COUNT(*) as n
   FROM app_sessions GROUP BY app_name ORDER BY min DESC LIMIT 10;"
```

## Utility Scripts

| Script | Purpose |
|---|---|
| `services/scripts/install-mlx-server-daemon.sh` | Install the MLX server as a launchd daemon (KeepAlive, auto-restart). |
| `services/scripts/uninstall-mlx-server-daemon.sh` | Stop and remove the MLX server daemon. |
| `scripts/refresh_pm_tasks.py` | Force-refresh `pm_tasks` from Jira without restarting the daemon. |
| `scripts/setup-hooks.sh` | Install git hooks (fmt + clippy pre-commit, full suite pre-push). |

```bash
# Install MLX server as a background daemon
bash services/scripts/install-mlx-server-daemon.sh [--port 7823]

# MLX server logs
tail -f ~/.meridian/logs/mlx-server.log

# Force-refresh Jira task cache
python3 scripts/refresh_pm_tasks.py

# Custom JQL or DB path
python3 scripts/refresh_pm_tasks.py --jql "project=KAN ORDER BY updated DESC"
python3 scripts/refresh_pm_tasks.py --db /path/to/meridian.db
```

## Development

After cloning + `./install.sh`, the four services run under launchd (see [Run](#run)). `meridian dev` starts a dev session from your checkout: backing services in the background, **the UI hot-reloading in your terminal**. (Source-checkout only — hidden on prebuilt installs.)

```bash
meridian dev            # backing services up (bg) + UI dev server foreground (hot reload)  ← start here
meridian dev daemon     # rebuild Rust + restart the daemon (bg)   ← backend loop (2nd terminal)
meridian dev ui         # UI dev server only — hot reload, foreground
meridian dev mlx        # restart only the MLX server (reloads the model, ~30s)
meridian dev screenpipe # restart only screenpipe
meridian dev build      # production build of daemon + UI (verify the shipped build; no run)
```

Typical session: run `meridian dev` in one terminal (UI hot-reloads on save at http://localhost:3939, Ctrl-C to stop); when you change Rust, run `meridian dev daemon` in a second terminal. The daemon's launchd agent runs `target/release/meridian` via a symlink, so a release build is picked up in place.

`meridian dev` / `meridian dev daemon` **leave the MLX model loaded** (fast) — only `meridian dev mlx` and `meridian restart` reload it, so avoid `meridian restart` in a tight loop. `meridian dev` waits for the MLX server to be ready before starting the daemon (which hard-exits if MLX is unreachable). Migrations apply automatically on daemon start. Note `meridian dev` stops the launchd dashboard to free the port for the dev server — bring the production dashboard back with `meridian start`.

To test one stage without the background daemon (hits the same `~/.meridian/meridian.db`):

```bash
cargo run --release -- worklog-status          # inspect the day's worklogs
cargo run --release -- pm-worklog --day DATE    # draft worklogs for a day
cargo run --release -- worklog-post-approved    # post approved worklogs now
```

> Don't run `cargo run` while the launchd daemon is up — two processes writing the DB. Stop it first (`meridian stop`) or just use `meridian dev daemon`.

### Quality checks (enforced by git hooks)

```bash
cargo fmt                       # format (pre-commit checks --check)
cargo clippy -- -D warnings     # lint — warnings are errors (pre-commit)
meridian dev build && cargo test   # build everything + run tests (pre-push runs the full suite)
bash scripts/setup-hooks.sh     # install the hooks after cloning
```

## Architecture

```
screenpipe.db (read-only)
       │
       ▼
  ETL runner  ──────────────────────────────────────────────────┐
  (every 60s)                                                   │
       │                                                         │
       ├─ get_frames_since(cursor)                              │
       ├─ detect app-switch boundaries                          │
       ├─ extract_block_context (OCR, audio, signals)           │
       ├─ upsert active_session (open block)                    │
       └─ close completed sessions → app_sessions               │
                                                                 ▼
                                                        meridian.db
                                                                 │
                                                                 ▼
  Classification ──────── POST /classify_sessions ─────► MLX server
  (per tick)              http://127.0.0.1:7823          (Qwen3.5-9B,
       │                                                  model in memory)
       └─ writes ticket_links, session_dimensions ────────────► meridian.db
```

The MLX server is a long-running FastAPI process managed by launchd. It loads the model once at startup — no cold-load penalty per session. The Rust daemon HTTP-calls it and writes the classification results back to `meridian.db`.

### Coding-agent pipeline

On machines that run a terminal coding agent (Claude Code / Codex), the **same Rust daemon** also turns those sessions into `app_sessions` rows and runs them through summarise → classify. It's gated — dormant if neither agent is present.

```
~/.claude/projects/*.jsonl                indexer task (≈10 min poll + SessionEnd hook)
~/.codex/sessions/**/*.jsonl  ──────────► parse → segment (1h time-box, split at a
                                          user-prompt boundary) → upsert app_sessions
                                                     │
                          coding_agent_live ─seal→ pending_summariser
                                                     │
   summariser task ──► claude -p / codex exec (2 tries) ─fail→ MLX /summarise ──► session_summary
                                                     │                            (source: claude|codex|mlx)
                                          pending_summariser → pending_classifier
                                                     │
   classifier (MLX drain) ──► /classify_sessions on the SUMMARY ──► task_key + dimensions
                                                     │
                                          pending_classifier → mlx_direct  (terminal)
```

Each session is sliced into ~1-hour segments (split at a real user-prompt boundary for continuity), sealed, summarised by its own agent (Claude for Claude sessions, Codex for Codex; MLX as the local fallback), then classified against your Jira tasks using the concise summary. The lifecycle is the `task_method` column: `coding_agent_live → pending_summariser → pending_classifier → mlx_direct`. The SessionEnd hook (`meridian coding-agent-hook`) seals a session immediately on `/clear` or stop; the 1-hour idle sweep is the backstop. Manual one-shots: `meridian coding-agent-summarise [--dry-run] [--day D]` and `meridian coding-agent-classify`.

## MCP Server

Meridian ships a TypeScript MCP server that exposes your session data to any MCP-compatible AI tool (Claude Code, Claude Desktop, Cursor, etc.).

`./install.sh` builds the MCP server into `packages/meridian-mcp/dist/index.js`. To rebuild it manually:

```bash
cd packages/meridian-mcp && npm run build
```

Add to your MCP client config (e.g. `~/Library/Application Support/Claude/claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "meridian": {
      "command": "node",
      "args": ["/path/to/meridian/packages/meridian-mcp/dist/index.js"]
    }
  }
}
```

Available tools: `get-sessions`, `get-timeline`, `get-stats`, `get-active-session`, `get-apps`, `search-sessions`, `get-session-detail`, `health-check`.

> **Note**: `audio_snippets` are stored in the DB but intentionally excluded from MCP tool responses to reduce LLM noise. They remain searchable via `search-sessions`.

The MCP server uses [sql.js](https://github.com/sql-js/sql.js) (pure WebAssembly SQLite) — no native Node.js modules, works with any Node.js version.

## License

MIT — see [LICENSE](LICENSE).
