//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

# Privacy Policy

**Last updated:** June 2026

## Overview

Meridian is a local-first developer-efficiency tool that turns your activity into structured work sessions and keeps your project management in sync. Capture and classification run on-device, and **Meridiana (the company) never receives your activity data** — there are no analytics servers and no telemetry by default.

Meridian does make network calls, but only ones you control: it sends the ticket updates you approve directly from your machine to the trackers *you* connect (Jira, GitHub, Linear). Each kind of outbound traffic is described below.

---

## Data Collection

Meridian itself collects **no data**. Instead:

- **screenpipe** (a separate process you control) captures screen frames, OCR text, audio transcriptions, and UI element metadata according to its own configuration
- **Meridian** reads screenpipe's local SQLite database (`~/.screenpipe/db.sqlite`), structures the raw captures into app-based activity sessions, and stores them in its own database (`~/.meridian/meridian.db`)
- **Processed on-device** — capture, structuring, and classification all happen locally; your screen content is never sent to Meridiana. The only data that leaves your machine is described under [Third-party Integrations](#third-party-integrations) and [Optional cloud LLM](#optional-cloud-llm) below

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

## Optional cloud LLM

By default, classification and summarisation run **on-device** using a local model (MLX). You can optionally configure a cloud LLM (any OpenAI-compatible endpoint, e.g. OpenRouter) as a fallback for machines that can't run the local model.

**If you enable this, session text — which may include OCR'd screen content — is sent to that provider** so it can classify the session. This is **off by default**; it only happens if you set a cloud LLM API key in your configuration. Choose a provider you trust, and review its data-handling policy. To keep everything on-device, leave the cloud LLM unconfigured.

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
