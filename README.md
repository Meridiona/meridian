<div align="center">

# Meridian

**Your project management — handled, quietly.**

[![CI](https://github.com/Meridiona/meridian/actions/workflows/ci.yml/badge.svg)](https://github.com/Meridiona/meridian/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Platform: macOS Apple Silicon](https://img.shields.io/badge/platform-macOS%20Apple%20Silicon-111111.svg)](#install)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-dea584.svg)](https://www.rust-lang.org/)

</div>

You finish something good — and then you have to go *log* it. Update the status. Write the standup. Drag the card. Meridian makes that second job disappear.

It runs quietly on your Mac, understands what you're working on, and keeps your tickets in **Jira, GitHub Issues, and Linear** current — so you never start a timer, fill out a form, or drag a card again.

Not a time tracker you fill out. Not a dashboard you check. A background layer that keeps your project management honest while you stay in the work.

## Demo

https://github.com/user-attachments/assets/501f41e6-aa89-404b-b430-a0b8b59c198e

---

## Why

Every week, hours vanish into the work *about* the work — the status updates, the time logs, the standups, the cards. None of it is hard. It's just relentless, and it pulls you out of flow every single time.

But everything needed to do it already exists in what you just did — the code you wrote, the PRs you reviewed, the branch you're on. Meridian reads that context, works out which task it belongs to, and puts the update where it goes. The busywork doesn't get faster. It gets gone.

- **Zero effort** — no timers, no forms, no prompts. It just runs.
- **On-device by default** — capture and classification happen locally. The only things that leave your machine are the ticket updates you approve, sent to your own trackers. ([Privacy](#privacy))
- **Correct by design** — a wrong task assignment is worse than no feature. Accuracy is the point.

## How it works

```
   capture            classify             sync
 your activity  →   which task is   →   your tracker
  (on-device)        this? (on-device)   (you approve)
```

1. **Capture** — Meridian bounds your activity into clean, app-based work sessions, accurate across sleep, idle, and restarts.
2. **Classify** — an on-device model labels each session and links it to the specific ticket it belongs to, using what's on screen, the branch you're on, and the tools in play.
3. **Sync** — the matching ticket in Jira / GitHub Issues / Linear is updated for you. **Nothing posts without your approval.**

A dashboard inside the Meridian app (open it from the menu-bar tray icon) shows your day as a timeline and per-app breakdown. A built-in [MCP server](SETUP.md#mcp-server) makes the same data available to AI tools like Claude and Cursor.

## Install

**Requirements:** macOS on Apple Silicon (M1+).

> The on-device model runs on [MLX](https://github.com/ml-explore/mlx) (Metal-based, arm64-only), so **Intel Macs are not supported** — the installer checks the hardware and refuses cleanly. **Windows and Linux are not supported** either: the capture and service stack is macOS-only. Rosetta toolchains (x86_64 terminal, Homebrew, or Python) on an Apple Silicon Mac are fine — Python services are built from a pinned, uv-managed arm64 interpreter regardless of what's on your `PATH`.

```bash
curl -fsSL https://raw.githubusercontent.com/Meridiona/meridian/main/scripts/bootstrap.sh | bash
```

This installs the `meridian` CLI and runs `meridian setup`, which brings up everything else — background services, the on-device model, and the dashboard — then walks you through macOS permissions and connecting your tracker.

Prefer npm:

```bash
npm install -g @meridiona/meridian
meridian setup
```

## Supported PM tools

Connect one or more trackers and Meridian maps captured work sessions to tasks, then posts time-logged worklogs as comments on the task.

| Tracker | Auth | Worklog mechanism | Cloud / on-prem |
|---|---|---|---|
| **Jira** | Browser OAuth (recommended) or Basic (URL + email + API token) | Native Jira worklog endpoint | Cloud (Atlassian) |
| **GitHub** | `gh` CLI token (no PAT needed) or classic PAT | Issue comment (no native time-tracking API) | Cloud only |
| **Linear** | Personal API key | Issue comment (no native time-tracking API) | Cloud only |
| **Trello** | Browser OAuth | Card comment (no native time-tracking API) | Cloud only |
| **Azure DevOps** | Personal Access Token (PAT) with Work Items Read & write scope | Work item comment (no native time-tracking API) | Cloud (`dev.azure.com`) + legacy (`*.visualstudio.com`) + on-premises (TFS/Azure DevOps Server) |

### Azure DevOps quick-start

Just two variables — paste your project URL from the browser and your PAT:

```bash
# Add to .env (or run meridian setup to be prompted interactively)
AZURE_DEVOPS_URL=https://dev.azure.com/your-org/your-project
AZURE_DEVOPS_PAT=your-pat-here
```

`AZURE_DEVOPS_URL` works for all three URL shapes: `dev.azure.com/org/project`, `org.visualstudio.com/project`, and on-premises servers — meridian auto-extracts the org and project.

To create a PAT: **User settings → Personal access tokens → New token**, scope: **Work Items → Read & write**.

```bash
# Verify tasks synced
meridian force-pm-sync
sqlite3 ~/.meridian/meridian.db "SELECT task_key, title FROM pm_tasks WHERE provider='azure_devops';"
```

## Data location
👉 **Full walkthrough — permissions, tracker setup, configuration, troubleshooting: [SETUP.md](SETUP.md).**

## Quickstart

```bash
meridian start     # bring everything up
meridian status    # check what's running
meridian logs -f   # watch the pipeline live
meridian doctor    # diagnose config / services / permissions
```

Open the dashboard from the Meridian tray icon in the menu bar. Stop everything with `meridian stop`.

> **Nothing posts to your tracker automatically.** Meridian *drafts* worklogs and ticket updates; you review and approve each one in the dashboard. Approval is the only gate.

## Privacy

Meridian is built to keep your data yours:

- **Capture and classification run on-device** — the local model (MLX) classifies your sessions; your screen content is not sent anywhere to do this.
- **Meridiana, the company, never receives your data.** There are no analytics servers and no default telemetry.
- **The only outbound traffic is yours, and you control it:** approved ticket updates go directly from your machine to the trackers *you* connect (Jira, GitHub, Linear), and integration tokens are stored locally.
- **Opt-in cloud LLM:** if you configure a cloud model instead of the local one, session text is sent to that provider — this is off by default.

Full detail: [docs/privacy.md](docs/privacy.md).

## Contributing

Meridian is open source and contributions are welcome — bug reports, fixes, docs, and features. See **[CONTRIBUTING.md](CONTRIBUTING.md)** to get a dev environment running, and **[CLAUDE.md](CLAUDE.md)** for architecture and conventions. By participating you agree to our [Code of Conduct](CODE_OF_CONDUCT.md).

## Build from source

```bash
git clone https://github.com/Meridiona/meridian
cd meridian
./install.sh
```

Builds the daemon and dashboard from source and registers the same services. See [SETUP.md](SETUP.md) for the development workflow, configuration reference, and MCP server setup.

## Built on

Meridian stands on excellent open-source work:

- [**screenpipe**](https://screenpi.pe) — the capture crates Meridian's in-process capture is forked from (pinned at the last MIT release, 0.4.6).
- [**Tauri**](https://tauri.app) — the framework that wraps the dashboard and tray into a single native app.
- [**MLX**](https://github.com/ml-explore/mlx) — Apple's framework powering the on-device model.

Thank you to these communities. 🙏

## License

MIT — see [LICENSE](LICENSE).
