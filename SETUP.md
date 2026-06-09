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
2. Connecting your issue tracker (Jira, Linear, or GitHub)

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

## Connect your tracker (Jira, Linear, or GitHub)

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

Worklogs are **never posted automatically** — Meridian drafts them for you to review and approve in the dashboard.

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

**Log targets:** `daemon`, `daemon-error`, `screenpipe`, `screenpipe-error`, `ui`, `ui-error`, `mlx-server`, `mlx-server-error`

---

## What's running

| Service | Role |
|---|---|
| **screenpipe** | captures screen activity (the data source) |
| **meridian daemon** | ETL pipeline, classification, coding-agent ingest, worklog drafting |
| **MLX server** | on-device model for classification and worklog synthesis |
| **dashboard** | web UI at http://localhost:3939 |

> **8 GB M1/M2 Air (macOS 26+):** the MLX server uses Apple Intelligence — no model download needed.
> Requires macOS 26 and Apple Intelligence enabled in System Settings.
>
> **16 GB+:** the first MLX start downloads the classifier model (~6 GB). Follow progress with `meridian logs mlx-server -f`.

---

## Where everything lives

| | Path |
|---|---|
| Config | `~/.meridian/app/.env` |
| Database | `~/.meridian/meridian.db` |
| Logs | `~/.meridian/logs/` |
| App bundle | `~/.meridian/app/` |

---

## Troubleshooting

- **Dashboard not loading** — wait ~15 s after start; check `meridian logs ui-error -f`
- **Empty Tasks board** — connect a tracker (see above), confirm with `meridian doctor`
- **No categories** — model still loading; run `meridian logs mlx-server -f` and wait for `server: MLX model ready`
- **`meridian: command not found`** — open a new terminal or run `source ~/.zshrc`
- **Gatekeeper blocks the binary** — run `xattr -dr com.apple.quarantine ~/.meridian/app`, then `meridian restart`

---

## Build from source

```bash
git clone https://github.com/Meridiona/meridian
cd meridian
cp .env.example .env
./install.sh
bash scripts/setup-hooks.sh
```

See `CLAUDE.md` for architecture, conventions, and common-task recipes.
