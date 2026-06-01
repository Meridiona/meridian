# Setting up Meridian

Meridian installs from **npm** — no repo to clone, no compiler to set up. The package ships a prebuilt binary; one `meridian setup` wires up the four background services.

> **Platform:** macOS on **Apple Silicon** (M1 or later). The on-device model needs Metal; Intel Macs aren't supported. (`npm` ships with Node — `brew install node` if you don't have it.)

---

## 1. Install

```bash
npm install -g @meridiona/meridian
meridian setup
```

`npm install` fetches the prebuilt app. `meridian setup` copies it to `~/.meridian/app`, installs any missing prerequisites (Homebrew packages, Python 3.11, screenpipe, ffmpeg), creates the Python environment + on-device model deps, and registers four launchd agents that start automatically:

| Service | Role |
|---|---|
| **screenpipe** | captures screen + audio (the data source) |
| **meridian daemon** | the pipeline — ETL, classification, coding-agent ingest, PM-worklog |
| **MLX server** | the on-device model (classification + worklog synthesis) |
| **dashboard** | the web UI at http://localhost:3000 |

> First setup downloads the ~6 GB model on first MLX start — give it a few minutes (`meridian logs mlx-server -f` shows progress).

Pin a specific version with `npm install -g @meridiona/meridian@0.3.0`.

---

## 2. Grant macOS permissions (required, manual)

screenpipe needs three permissions macOS will only let **you** grant. The installer opens each pane; for each:

1. **Screen Recording** — System Settings → Privacy & Security → Screen Recording → click **+**, add the screenpipe binary, toggle ON.
2. **Accessibility** — same, under Accessibility.
3. **Microphone** — screenpipe appears here after it first tries the mic; toggle ON.

After granting, run `meridian restart`.

---

## 3. Connect Jira (optional, but it's the point)

Meridian drafts worklogs against your Jira tickets. To enable:

1. Create an API token at **https://id.atlassian.com/manage-profile/security/api-tokens**.
2. Open the config and add three lines:
   ```bash
   meridian config edit
   ```
   ```dotenv
   JIRA_BASE_URL=https://your-org.atlassian.net
   JIRA_EMAIL=you@your-org.com
   JIRA_API_TOKEN=your-api-token
   ```
3. Restart: `meridian restart`.

> GitHub and Linear are also supported (`GITHUB_TOKEN`/`GITHUB_ORG`, `LINEAR_API_KEY`/`LINEAR_TEAM_IDS`) — same file.

**Worklog posting is OFF by default** — Meridian *drafts* worklogs but never writes to Jira until you opt in:
```dotenv
PM_WORKLOG_POST_ENABLED=true
```
Review the drafts first (`meridian worklog-status`), then enable when you're ready.

---

## 4. Verify it's running

```bash
meridian status          # all four services
meridian doctor          # diagnose config / services / permissions
meridian logs -f         # watch the pipeline live
open http://localhost:3000
```

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
meridian update      # npm-install the latest release + re-run setup (keeps your .env)
meridian uninstall   # stop services + remove the CLI (your data in ~/.meridian/ is kept)
```

---

## Privacy

Everything runs **on your machine**. screenpipe records your screen and audio locally into `~/.screenpipe/`; Meridian reads that and writes to `~/.meridian/meridian.db`. There is no telemetry by default and no outbound network — the **only** thing that ever leaves your Mac is a Jira worklog, and only after you set `PM_WORKLOG_POST_ENABLED=true`.

---

## Troubleshooting

- **Dashboard not loading** — give it ~15 s after start; check `meridian logs ui-error -n 50`.
- **No worklogs / classifications** — confirm the model is up: `meridian logs mlx-server -f` should show `MLX model ready`; `curl -s localhost:7823/health`.
- **`meridian: command not found`** — ensure `~/.local/bin` is on your `PATH`.
- **Gatekeeper blocks the binary** (unsigned builds) — `xattr -dr com.apple.quarantine ~/.meridian/app`, then `meridian restart`.
- **Moved the install?** — the services point at `~/.meridian/app`; don't move it. Re-run the installer if you must relocate.

---

## Build from source (contributors)

Developers working on Meridian itself clone and build instead:

```bash
git clone https://github.com/Meridiona/meridian
cd meridian
./install.sh          # builds the daemon + UI, sets up services
bash scripts/setup-hooks.sh   # pre-commit/pre-push hooks
```

`./install.sh --no-mlx` uses the hermes LLM-selector backend instead of the MLX server.
