// meridian — normalises screenpipe activity into structured app sessions

# Privacy Policy

**Last updated:** June 2026

## Overview

Meridian is a local-first activity-tracking daemon that normalizes screen-capture frames from screenpipe into structured app sessions. All data is processed and stored locally on your machine — Meridiana does not collect, transmit, or store your activity data on any remote server.

---

## Data Collection

Meridian itself collects **no data**. Instead:

- **screenpipe** (a separate process you control) captures screen frames, OCR text, audio transcriptions, and UI element metadata according to its own configuration
- **Meridian** reads screenpipe's local SQLite database (`~/.screenpipe/db.sqlite`), normalizes the raw captures into app-based activity sessions, and stores them in its own database (`~/.meridian/meridian.db`)
- **Your machine only** — all processing happens locally; nothing leaves your computer

---

## Data Storage

- **Activity sessions** are stored in `~/.meridian/meridian.db` (SQLite, on your local disk)
- **OAuth tokens** (for Jira, Linear, GitHub integrations) are stored in `~/.meridian/oauth/` (mode 0600, encrypted by your OS keychain where available)
- **Logs** are stored in `~/.meridian/logs/`
- Your API tokens (if using legacy API-key auth instead of OAuth) are stored in `~/.meridian/app/.env` (plain text, only readable by you)

**You own all this data.** You can delete it, export it, or migrate it at any time.

---

## Third-party Integrations

Meridian can optionally connect to:

- **Jira** (Atlassian) — to post worklogs. Connection is OAuth 2.0; tokens are stored locally and never sent to Meridiana
- **Linear** — to post worklogs. API keys are stored locally and never sent to Meridiana
- **GitHub** — to post worklog comments. Tokens are stored locally and never sent to Meridiana

When you authorize an integration:
1. You log into the third-party service (your browser, your credentials)
2. The OAuth token or API key is stored **only on your machine**
3. Meridian makes API calls **directly from your machine** to the third-party service using that token
4. Meridiana (the company) never sees your credentials, tokens, or the data you exchange

---

## What Meridiana Collects (if you opt-in to telemetry)

If you enable telemetry in Meridian's configuration:
- **Aggregated usage metrics** — which features you use, error rates, performance timings
- **No personal data** — no screen content, no window titles, no activity details
- **OpenObserve OTLP endpoint** (if configured) — telemetry is sent to your specified observability backend, not to Meridiana by default

---

## Your Rights

- **Access** — all your data is in plain-text SQLite files on your machine; you can inspect or export it anytime
- **Delete** — run `rm ~/.meridian/meridian.db` or `meridian uninstall` to remove all local data
- **Portability** — export your activity data or switch to another tool; there's no vendor lock-in
- **No tracking** — Meridian does not track you across devices or sessions

---

## Contact

For privacy questions or concerns, reach out to **akarsh@meridiona.com**.

---

## Changes to This Policy

We may update this policy from time to time. If we make material changes, we'll notify you (by updating this file in the repository).
