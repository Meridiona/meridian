# Meridian Setup

**Platform:** macOS, Apple Silicon (M1 or later). Intel Macs are not supported.

---

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/Meridiona/meridian/main/scripts/bootstrap.sh | bash
```

This installs Node if missing, installs the `meridian` CLI, then runs `meridian setup` automatically.

`meridian setup` will walk you through:
1. Granting **Screen Recording** and **Accessibility** permissions to screenpipe (required)
2. Connecting your issue tracker (Jira, Linear, GitHub, Trello, or Azure DevOps)

---

## Grant macOS permissions

screenpipe needs two permissions you must grant manually:

1. **Screen Recording** — System Settings → Privacy & Security → Screen Recording → click **+**, add screenpipe, toggle ON
2. **Accessibility** — same pane, under Accessibility

After granting both, run:

```bash
meridian restart
```

---

## Connect your tracker (Jira, Linear, GitHub, Trello, or Azure DevOps)

Edit the config file:

```bash
meridian config edit
```

Add the block for your tracker, then `meridian restart`.

### Jira

The installer now connects Jira for you (it offers browser sign-in during `meridian setup` / `./install.sh`). To connect manually or re-connect:

> **Maintainers:** the browser flow depends on a one-time Atlassian app registration — including the **Distributable** toggle that gates all non-Meridiona users, and the `JIRA_OAUTH_CLIENT_SECRET` Actions secret the release build needs. The registration essentials are documented inline at `DEFAULT_CLIENT_ID` in `src/intelligence/oauth/jira.rs`.

**Easiest — browser OAuth (no API token):**
```bash
meridian oauth-login jira    # opens your browser → click Accept
meridian restart
```
Tokens are saved to `~/.meridian/oauth/jira.json` and auto-refresh; the site is discovered automatically. Nothing to put in `.env`. Each user connects to **their own** Jira site (discovered from who signs in).

> **If your Atlassian org blocks third-party apps:** some orgs require an admin to approve OAuth apps (you'll see "your site admin must authorize this app") or disable user app installs entirely. In that case, use the **API-token fallback** below instead — it needs no org-level app approval.

**Legacy — static API token** (use this *instead* of the OAuth login):

1. Create an API token at https://id.atlassian.com/manage-profile/security/api-tokens
2. Add to config:
```dotenv
JIRA_BASE_URL=https://your-org.atlassian.net
JIRA_EMAIL=you@your-org.com
JIRA_API_TOKEN=your-api-token
```

### Linear

1. Create a personal API key at https://linear.app/settings/account/security
2. Add to config:
```dotenv
LINEAR_API_KEY=lin_api_your_key_here
```

### GitHub

**Easiest:** if you have the `gh` CLI installed and authenticated, `meridian setup` extracts the token automatically and shows a list of your GitHub Projects to pick from.

**Manual:** create a personal access token (classic) at https://github.com/settings/tokens/new
- Required scopes: `repo`, `read:org`, `read:project`
- `read:project` lets meridian read your GitHub Projects; `repo` posts worklog comments on issues

Add to config:
```dotenv
GITHUB_TOKEN=ghp_your_token
GITHUB_PROJECT_IDS=PVT_xxx,PVT_yyy
```

To find your project node ID: `gh api graphql -f query='{ viewer { projectsV2(first: 10) { nodes { id title } } } }'`

### Trello

The installer offers Trello browser sign-in during `meridian setup`. To connect manually or re-connect:

```bash
meridian oauth-login trello    # opens your browser → click Allow
meridian restart
```

Your token is saved to `~/.meridian/oauth/trello.json`. Meridian syncs open cards assigned to you across all your boards.

Worklogs are **never posted automatically** — Meridian drafts them for you to review and approve in the dashboard.

### Azure DevOps

Just two things needed: your project URL and a PAT.

**Step 1 — Project URL**

Open your Azure DevOps project in a browser and copy the URL from the address bar. It will look like one of these:

```
Cloud (standard):  https://dev.azure.com/mycompany/MyProject
Cloud (legacy):    https://mycompany.visualstudio.com/MyProject
On-premises:       https://tfs.corp.com/DefaultCollection/MyProject
```

**Step 2 — Personal Access Token**

Go to **User settings (avatar, top-right) → Personal access tokens → New token**.
- Required scope: **Work Items → Read & write**
- Choose org-scoped (not global) — global PATs are being retired by Microsoft

**Step 3 — Add to config**

```dotenv
AZURE_DEVOPS_URL=https://dev.azure.com/mycompany/MyProject   ← paste your URL here
AZURE_DEVOPS_PAT=your-pat-here
```

Run `meridian config edit` to open the config file, add the two lines, save, then:

```bash
meridian restart
```

Meridian syncs all work items assigned to you that are not completed. State filtering is dynamic — custom state names (any board workflow) are handled automatically.

> You can also run `meridian setup` and choose **Azure DevOps** to be prompted interactively instead of editing the file manually.

---

## Commands

```bash
meridian start              # start all services
meridian stop               # stop all services
meridian restart            # restart all services
meridian status             # show status of all services
meridian doctor             # diagnose config, services, and permissions
meridian config edit        # open the config file in your editor
meridian logs               # tail the daemon log
meridian logs -f            # follow the daemon log live
meridian logs <target>      # tail a specific service log
meridian logs <target> -f   # follow a specific service log live
meridian worklog-status     # show pending worklog drafts
meridian version            # show installed version
meridian update             # update to the latest release
meridian uninstall          # stop services and remove the CLI
```

**Log targets:** `daemon`, `daemon-error`, `screenpipe`, `screenpipe-error`, `mlx-server`, `mlx-server-error`

---

## What's running

| Service | Role |
|---|---|
| **screenpipe** | captures screen activity (the data source) |
| **meridian daemon** | ETL pipeline, classification, coding-agent ingest, worklog drafting |
| **MLX server** | on-device model for classification and worklog synthesis |
| **dashboard** | embedded in the Meridian tray app (open from the menu-bar icon) |

> **8 GB M1/M2 Air (macOS 26+):** the MLX server uses Apple Intelligence — no model download needed.
> Requires macOS 26 and Apple Intelligence enabled in System Settings.
>
> **16 GB+:** the first MLX start downloads the classifier model (~6 GB). Follow progress with `meridian logs mlx-server -f`.

---

## Where everything lives

| | Path |
|---|---|
| Config | `~/.meridian/.env` |
| Database | `~/.meridian/meridian.db` |
| Logs | `~/.meridian/logs/` |
| App bundle | `~/.meridian/app/` |

---

## Troubleshooting

- **Dashboard not loading** — the dashboard is bundled inside the tray app; quit and relaunch the Meridian tray (a stale instance can show blank)
- **Empty Tasks board** — connect a tracker (see above), confirm with `meridian doctor`
- **No categories** — model still loading; run `meridian logs mlx-server -f` and wait for `server: MLX model ready`
- **`meridian: command not found`** — open a new terminal or run `source ~/.zshrc`
- **Gatekeeper blocks the binary** — run `xattr -dr com.apple.quarantine ~/.meridian/app`, then `meridian restart`

---

## Configuration

All settings are environment variables in `~/.meridian/.env`; defaults work out of the box. The installer collects the credential-bearing ones interactively.

| Variable | Default | Description |
|---|---|---|
| `MERIDIAN_DB` | `~/.meridian/meridian.db` | Where Meridian writes its database |
| `POLL_INTERVAL_SECS` | `60` | How often to check for new activity |
| `CLASSIFICATION_ENABLED` | `true` | Enable session → task classification (set `false` to run capture + categorisation only, no model needed) |
| `MLX_SERVER_PORT` | `7823` | Port the on-device model server listens on |
| `CLASSIFICATION_TIMEOUT_S` | `120` | Per-session inference timeout |
| `MERIDIAN_UI_PORT` | `3939` | Dev-only: `next dev` / Tauri devUrl port (no effect on installed builds — the dashboard is bundled in the tray) |

Edit with `meridian config edit`, then `meridian restart`.

---

## MCP server

Meridian ships a TypeScript MCP server that exposes your session data to any MCP-compatible AI tool (Claude Code, Claude Desktop, Cursor, …). It's built into `packages/meridian-mcp/dist/index.js` during install.

Add it to your MCP client config:

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

Tools: `get-sessions`, `get-timeline`, `get-stats`, `get-active-session`, `get-apps`, `search-sessions`, `get-session-detail`, `health-check`. It uses [sql.js](https://github.com/sql-js/sql.js) (pure-WASM SQLite), so it works with any Node.js version.

---

## Build from source

```bash
git clone https://github.com/Meridiona/meridian
cd meridian
cp .env.example .env
./install.sh
bash scripts/setup-hooks.sh
```

### Development

After `./install.sh`, the services run under launchd. `meridian dev` starts a dev session from your checkout — backing services in the background, the UI hot-reloading in your terminal (source-checkout only).

```bash
meridian dev            # backing services (bg) + UI dev server foreground (hot reload)
meridian dev daemon     # rebuild Rust + restart the daemon (bg)
meridian dev ui         # UI dev server only — hot reload
meridian dev mlx        # restart only the model server (reloads the model, ~30s)
meridian dev build      # production build of daemon + UI (no run)
```

Typical loop: `meridian dev` in one terminal; when you change Rust, `meridian dev daemon` in a second. Avoid `meridian restart` in a tight loop — it reloads the model.

See `CLAUDE.md` for architecture, conventions, and common-task recipes.
