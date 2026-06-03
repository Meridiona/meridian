# Setting up Meridian

Meridian installs from **npm** — no repo to clone, no compiler to set up. The package ships a prebuilt binary; one `meridian setup` wires up the four background services.

> **Platform:** macOS on **Apple Silicon** (M1 or later). The on-device model needs Metal; Intel Macs aren't supported. (`npm` ships with Node — `brew install node` if you don't have it.)

---

## 1. Install

```bash
npm install -g @meridiona/meridian
meridian setup
```

`npm install` downloads the prebuilt app (~170 MB — includes a pre-built Python environment so setup is fast). `meridian setup` copies it to `~/.meridian/app`, installs any missing prerequisites (Homebrew packages, Python 3.11, screenpipe, ffmpeg), extracts the pre-built Python environment, and registers four launchd agents that start automatically:

> **EACCES error on `npm install`?** Your npm prefix is root-owned (stock macOS Node). Use the one-line installer instead — it fixes the prefix automatically:
> ```bash
> curl -fsSL https://raw.githubusercontent.com/Meridiona/meridian/main/scripts/bootstrap.sh | bash
> ```

| Service | Role |
|---|---|
| **screenpipe** | captures the screen (the data source; audio disabled) |
| **meridian daemon** | the pipeline — ETL, classification, coding-agent ingest, PM-worklog |
| **MLX server** | the on-device model (classification + worklog synthesis) |
| **dashboard** | the web UI at http://localhost:3939 (override via `MERIDIAN_UI_PORT`) |

> First setup downloads the ~6 GB model on first MLX start — give it a few minutes (`meridian logs mlx-server -f` shows progress).

Pin a specific version with `npm install -g @meridiona/meridian@0.3.0`.

---

## 2. Grant macOS permissions (required, manual)

screenpipe needs two permissions macOS will only let **you** grant. The installer opens each pane; for each:

1. **Screen Recording** — System Settings → Privacy & Security → Screen Recording → click **+**, add the screenpipe binary, toggle ON.
2. **Accessibility** — same, under Accessibility.

(Audio capture is disabled, so no Microphone permission is needed.)

After granting, run `meridian restart`.

---

## 3. Connect your issue tracker (optional, but it's the point)

**What works without a tracker:** the activity timeline, app breakdown, and session categories all populate from your screen activity alone — connect nothing and the dashboard still shows how you spent your day.

**What a tracker unlocks:** the **Tasks** board (your assigned tickets, with time mapped to each) and **worklog drafting**. This is the reason Meridian exists.

Meridian drafts worklogs against the tickets you're assigned. It supports **Jira, Linear, and GitHub** — pick the one you use (you can configure more than one, but most people use a single tracker). Open the config with `meridian config edit` and add the block for your tracker, then `meridian restart`. The dashboard's Tasks tab also shows a per-tracker "connect" guide with these same steps.

### Jira

1. Create an API token at **https://id.atlassian.com/manage-profile/security/api-tokens**.
2. Add:
   ```dotenv
   JIRA_BASE_URL=https://your-org.atlassian.net
   JIRA_EMAIL=you@your-org.com
   JIRA_API_TOKEN=your-api-token
   # JIRA_PROJECT_KEYS=KAN,ENG   # optional filter; empty = all projects
   ```

Meridian syncs the issues assigned to you (`assignee = currentUser()`, not Done) and logs time via Jira's native **worklog** API (time spent + a comment), the way Jira time tracking is meant to work.

### Linear

1. Create a **personal API key** at **https://linear.app/settings/account/security** → *Personal API keys*.
2. Add:
   ```dotenv
   LINEAR_API_KEY=lin_api_your_key_here
   # LINEAR_TEAM_IDS=ENG,DESIGN   # optional filter by team key or id; empty = all teams
   ```

Meridian syncs the issues assigned to you. **Linear has no native time-tracking API**, so a worklog is posted as a structured comment on the issue — a "⏱ Worklog — 1h 30m" line plus the synthesised narrative — which is Linear's only first-class, per-issue, timestamped record.

### GitHub

1. Create a token at **Settings → Developer settings → Personal access tokens**. A classic token with the **`repo`** scope is the simplest and works for both personal and org issues. (Fine-grained tokens work too: grant **Issues: Read and write** on the repos you care about.)
2. Add:
   ```dotenv
   GITHUB_TOKEN=ghp_your_token
   GITHUB_ORG=your-org          # the owner whose issues to track (org or your username)
   # GITHUB_REPOS=your-org/api,your-org/web   # optional filter; empty = all repos under the owner
   ```

Meridian syncs the open issues assigned to you under that owner. **GitHub has no native time tracking**, so a worklog is posted as a structured comment on the issue (an append-only "⏱ Worklog" ledger entry), which is visible on the issue and on any Project board it belongs to.

---

**Nothing posts automatically — for any tracker.** Meridian *drafts* a worklog for each task/hour; you review, edit, and approve each one in the dashboard's **Worklogs** view, and the daemon posts approved worklogs within ~60s. Approval is the only gate — there is no auto-post switch. Check the day's drafts any time with `meridian worklog-status`.

---

## 4. Verify it's running

```bash
meridian status          # all four services
meridian version         # installed version
meridian doctor          # diagnose config / services / permissions
meridian logs -f         # watch the pipeline live
open http://localhost:3939
```

### What to expect on first run

1. **Model download (~6 GB, once).** The first MLX start pulls the model — a few minutes. `meridian logs mlx-server -f` ends with `server: MLX model ready`. Until then, classification just waits.
2. **Activity shows within a couple of minutes.** screenpipe captures continuously and the daemon runs ETL every 60 s, so the timeline and app breakdown fill in shortly after setup. Categories appear once the model is ready and a session has enough content to classify.
3. **Tasks & worklogs need a tracker.** The Tasks board stays empty until you complete step 3; the dashboard's Tasks tab shows exactly what to add.
4. **Nothing posts on its own** — worklogs are drafted for you to review and approve (see below).

---

## Where everything lives

| | Path |
|---|---|
| App (prebuilt bundle) | `~/.meridian/app` |
| Config | `~/.meridian/app/.env` (the one backend config) |
| Database | `~/.meridian/meridian.db` (yours — plain SQLite) |
| Logs | `~/.meridian/logs/` (per service: `daemon.log`, `mlx-server.log`, `screenpipe.log`, `ui.log`, plus `*-error.log`) |
| Services (launchd) | `~/Library/LaunchAgents/com.meridiona.*.plist` |

**Logs:** `meridian logs <target> [-f]` — targets: `daemon`, `daemon-error`, `mlx-server`, `mlx-server-error`, `screenpipe`, `ui`. The `-error` variants show only warnings/errors.

---

## Update / uninstall

```bash
meridian update      # download latest release (~170 MB) + re-run setup (keeps your .env and database)
meridian uninstall   # stop services + remove the CLI (your data in ~/.meridian/ is kept)
```

---

## Privacy

Everything runs **on your machine**. screenpipe records your screen locally into `~/.screenpipe/` (audio capture is disabled); Meridian reads that and writes to `~/.meridian/meridian.db`. There is no telemetry by default and no outbound network — the **only** thing that ever leaves your Mac is a worklog (to Jira, Linear, or GitHub), and only one you explicitly approved in the dashboard.

---

## Troubleshooting

- **Dashboard not loading** — give it ~15 s after start; check `meridian logs ui-error -n 50`.
- **Empty Tasks board / no worklogs** — you need a connected tracker (§3). Confirm with `meridian doctor`; the dashboard's Tasks tab also shows what's missing.
- **No classifications / categories** — confirm the model is up: `meridian logs mlx-server -f` should end with `server: MLX model ready`; `curl -s localhost:7823/health`.
- **`meridian: command not found`** — ensure the npm bin directory is on your `PATH`. With Homebrew Node: `~/.local/bin`. With the bootstrap installer (system Node): `~/.npm-global/bin`. Add the missing path to `~/.zshrc` and run `source ~/.zshrc` (or open a new terminal).
- **`meridian update` says "unknown command"** — an older install left the CLI ahead of the launcher on your `PATH`. `source ~/.zshrc` (or open a new terminal); if it persists, reinstall with `npm install -g @meridiona/meridian`.
- **Gatekeeper blocks the binary** (unsigned builds) — `xattr -dr com.apple.quarantine ~/.meridian/app`, then `meridian restart`.
- **Moved the install?** — the services point at `~/.meridian/app`; don't move it. Re-run the installer if you must relocate.

---

## Build from source (contributors)

Developers working on Meridian itself clone and build instead of installing from npm:

```bash
git clone https://github.com/Meridiona/meridian
cd meridian
cp .env.example .env          # then fill in tracker creds (see §3) — one config for Rust + Python
./install.sh                  # builds the daemon + UI, sets up the four launchd services
bash scripts/setup-hooks.sh   # install the pre-commit / pre-push git hooks (required)
```

Classification runs on the persistent MLX inference server (Apple Silicon); `install.sh` sets it up automatically. Use `--mlx-port N` to change its port (default 7823).

### Day-to-day

```bash
cargo build --release                 # daemon (SQLX_OFFLINE is set automatically)
cargo test                            # Rust tests — must pass before committing
cargo clippy -- -D warnings           # lint — warnings are errors
cargo fmt                             # format
(cd ui && npm install && npm run dev) # dashboard in dev mode
```

The git hooks enforce the rules so CI doesn't have to: `pre-commit` runs fmt + clippy, `pre-push` runs the full suite (fmt + clippy + `cargo test` + UI build + UI tests). Never skip them with `--no-verify`.

- **`CLAUDE.md`** — repository layout, architecture, coding conventions, and the common-task recipes (add a DB query, an ETL signal, a UI route, an MCP tool, a classifier eval golden).
- **`TESTING.md`** — the ETL/DB invariants the integration tests enforce; read it before touching ETL logic, schema, or migrations.
- **`VISION.md`** — product decisions and the "why".
- **`services/agents/README.md`** — the classifier/worklog deep reference (classification logic, scoring, prompt-tuning recipes).
