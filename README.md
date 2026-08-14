<div align="center">

# Meridian

**Your project management — handled, quietly.**

[![CI](https://github.com/Meridiona/meridian/actions/workflows/ci.yml/badge.svg)](https://github.com/Meridiona/meridian/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Platform: macOS | Windows](https://img.shields.io/badge/platform-macOS%20%7C%20Windows-111111.svg)](#install)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-dea584.svg)](https://www.rust-lang.org/)

</div>

You finish something good — and then you have to go *log* it. Update the status. Write the standup. Drag the card. Meridian makes that second job disappear.

It runs quietly on your Mac or PC, understands what you're working on, and keeps your tickets in **Jira, GitHub Issues, Linear, Trello, and Azure DevOps** current — so you never start a timer, fill out a form, or drag a card again.

Not a time tracker you fill out. It's a private timeline of your day - browse it anytime to see what you actually did, task by task - plus a background layer that keeps your project management honest while you stay in the work.

> **Early access.** Meridian is young and shipping fast. The core loop — capture, classify, draft, approve — works every day on real projects, but you will hit rough edges. [Bug reports](https://github.com/Meridiona/meridian/issues/new?template=bug_report.yml) are genuinely welcome, and they get fixed quickly.

## Demo

https://github.com/user-attachments/assets/501f41e6-aa89-404b-b430-a0b8b59c198e

<!-- SCREENSHOTS: uncomment once docs/images/timeline.png and docs/images/approval.png exist.
     See docs/images/README.md for what to capture. Do not uncomment before the files land -
     GitHub renders a missing image as broken alt text.
|  |  |
|---|---|
| ![Your day as a timeline](docs/images/timeline.png) | ![Review and approve each worklog](docs/images/approval.png) |
| **Your day, reconstructed** - every session bounded and labelled, no timers. | **Nothing posts without you** - every draft waits for your approval. |
-->


---

## Why

Every week, hours vanish into the work *about* the work — the status updates, the time logs, the standups, the cards. None of it is hard. It's just relentless, and it pulls you out of flow every single time.

But everything needed to do it already exists in what you just did — the code you wrote, the PRs you reviewed, the branch you're on. Meridian reads that context, works out which task it belongs to, and puts the update where it goes. The busywork doesn't get faster. It gets gone.

- **Zero effort** — no timers, no forms, no prompts. It just runs.
- **Local-first** — capture, OCR, and session structuring happen on your machine. The AI that summarises your work and matches it to tickets runs through the coding-agent CLI you already use (Claude, Codex, Cursor, Copilot) or a cloud LLM you choose; the ticket updates you approve go straight to your own trackers. ([Privacy](#privacy))
- **Correct by design** — a wrong task assignment is worse than no feature. Accuracy is the point.

## How it works

```
   capture            classify             sync
 your activity  →   which task is   →   your tracker
  (on-device)        this?               (you approve)
```

1. **Capture** — Meridian bounds your activity into clean, app-based work sessions, accurate across sleep, idle, and restarts.
2. **Classify** — Meridian labels each session and links it to the specific ticket it belongs to, using what's on screen, the branch you're on, and the tools in play. The matching runs through the AI CLI you already use (or a cloud LLM you configure).
3. **Sync** — the matching ticket in Jira / GitHub Issues / Linear is updated for you. **Nothing posts without your approval.**

A dashboard inside the Meridian app (open it from the menu-bar tray icon) shows your day as a timeline and per-app breakdown. A built-in [MCP server](SETUP.md#mcp-server) makes the same data available to AI tools like Claude and Cursor.

## Install

**Requirements:** macOS (Apple Silicon or Intel), or Windows 10/11 (x64).

> On macOS Meridian ships as a universal binary, so Apple Silicon and Intel are both native. The local embedder is CPU-only (not Metal-accelerated), so it behaves identically everywhere. **Linux is not supported.**
>
> Windows support is newer than macOS and one capability differs: notifications are plain title-and-body toasts, without the action buttons macOS shows, so a notification asking you to confirm something can be read but not answered from the toast itself. Everything else - capture, the timeline, tracker sync, auto-update - works the same.

**macOS**

```bash
curl -fsSL https://raw.githubusercontent.com/Meridiona/meridian/main/scripts/bootstrap.sh | bash
```

**Windows** (PowerShell)

```powershell
irm https://raw.githubusercontent.com/Meridiona/meridian/main/scripts/bootstrap.ps1 | iex
```

This downloads the latest Meridian build, installs it, and launches it. The app stages its own background daemon on first run and opens the setup wizard — permissions (macOS only; Windows needs none), the AI that writes your summaries, and connecting your tracker. Updates install themselves from within the app.

Prefer to do it by hand? From the [latest release](https://github.com/Meridiona/meridian/releases/latest), download `Meridian.dmg` (macOS) and drag **Meridian** to Applications, or `Meridian-setup.exe` (Windows) and run it.

## Supported PM tools

Connect one or more trackers and Meridian maps captured work sessions to tasks, then posts time-logged worklogs as comments on the task.

| Tracker | Auth | Worklog mechanism | Cloud / on-prem |
|---|---|---|---|
| **Jira** | Browser OAuth (recommended) or Basic (URL + email + API token) | Native Jira worklog endpoint | Cloud (Atlassian) |
| **GitHub** | `gh` CLI token (no PAT needed) or classic PAT | Issue comment (no native time-tracking API) | Cloud only |
| **Linear** | Personal API key | Issue comment (no native time-tracking API) | Cloud only |
| **Trello** | Browser OAuth | Card comment (no native time-tracking API) | Cloud only |
| **Azure DevOps** | Personal Access Token (PAT) with Work Items Read & write scope | Work item comment (no native time-tracking API) | Cloud (`dev.azure.com`) + legacy (`*.visualstudio.com`) + on-premises (TFS/Azure DevOps Server) |

Connecting a tracker takes a couple of minutes each — step-by-step instructions for all five are in [SETUP.md](SETUP.md#connect-your-tracker-jira-linear-github-trello-or-azure-devops), or run `meridian setup` to be prompted interactively.

## Where your data lives

Everything Meridian records stays in `~/.meridian/`:

| Path | What's in it |
|---|---|
| `~/.meridian/meridian.db` | Your activity sessions and captured text. Encrypted at rest (SQLCipher); the key is generated on first run and kept in your OS keychain. |
| `~/.meridian/oauth/` | Tracker OAuth tokens, mode `0600`. |
| `~/.meridian/.env` | Tracker API keys and daemon configuration, mode `0600`. |
| `~/.meridian/telemetry/` | Local logs and traces. Read them with `meridian logs`. |

Raw captured frames are pruned automatically after 30 days. To remove everything, run `meridian uninstall`.

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

- **Capture and session structuring run on-device** — screen capture, OCR, and categorization happen locally, and the resulting database is encrypted at rest with a key held in your OS keychain. Your screen content, window titles, and captured text are never sent to Meridiana.
- **Approved ticket updates go straight from your machine** to the trackers *you* connect (Jira, GitHub, Linear, Trello, Azure DevOps). Integration tokens stay local; nothing is proxied through us.
- **AI summaries use the provider you choose:** matching work to tickets and drafting worklogs run through the coding-agent CLI you already use (Claude, Codex, Cursor, Copilot) or a cloud LLM you configure, so session text is sent to that provider. There is no on-device generative model.
- **Error reports are the one thing we receive, and you can switch them off.** Packaged builds send error-level diagnostics to help us fix crashes — **on by default**, off in one click at **Settings → Capture & Privacy**. Only WARN-and-above logs and failed traces qualify; content-bearing fields are stripped on your device before anything is sent, and your hostname is replaced by a one-way pseudonym. Screen content, OCR text, window titles, URLs, and LLM prompts never leave your machine either way. Builds compiled from source send nothing at all.
- **No analytics, no usage metrics, no cross-device tracking.**

Full detail, including exactly what an error report can and cannot contain: [docs/privacy.md](docs/privacy.md).

## Contributing

Meridian is open source and contributions are welcome — bug reports, fixes, docs, and features.

- **[CONTRIBUTING.md](CONTRIBUTING.md)** — get a dev environment running and open your first PR.
- **[ARCHITECTURE.md](ARCHITECTURE.md)** — how the system fits together, and why it's built this way.
- **[Good first issues](https://github.com/Meridiona/meridian/issues?q=is%3Aissue+is%3Aopen+label%3A%22good+first+issue%22)** — scoped starting points.
- **[Discussions](https://github.com/Meridiona/meridian/discussions)** — questions, ideas, and how you're using it.

By participating you agree to our [Code of Conduct](CODE_OF_CONDUCT.md).

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
- [**candle**](https://github.com/huggingface/candle) — the Rust ML framework running Meridian's local text-embedding model.

Thank you to these communities. 🙏

## License

MIT — see [LICENSE](LICENSE).
