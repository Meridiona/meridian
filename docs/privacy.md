//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

# Privacy Policy

**Last updated:** August 2026

## Overview

Meridian is a local-first developer tool that turns your activity into structured work sessions and keeps your project management in sync. Screen capture, OCR, and session structuring run entirely on your machine and are stored in an encrypted local database. Your screen content, window titles, and captured text never leave your device.

Meridian does make network calls. There are exactly three kinds, and you control all of them:

1. **Ticket updates you approve** — sent directly from your machine to the trackers you connect.
2. **AI summarisation** — session text sent to the LLM provider you choose.
3. **Error reports** — redacted, error-only diagnostics sent to Meridiana. **On by default in packaged installs; one switch turns it off.**

Each is described in full below. If you only read one section, read [Error reporting](#error-reporting).

---

## Data collection

Meridian captures activity itself — there is no separate capture process anymore:

- **Capture runs in-process** inside the Meridian menu-bar app: screen frames, OCR text, and accessibility-tree metadata, plus input events used only to tell activity from idle.
- **Everything is written locally** to `~/.meridian/meridian.db`, where the ETL pipeline structures raw captures into app-based activity sessions.
- **Structuring and categorization happen on-device.** Your screen content is never sent to Meridiana.

Session text *is* sent to the LLM provider you configure when summarising work and matching it to tickets — see [LLM provider](#llm-provider-summarisation--ticket-matching).

Meridian does not capture audio.

---

## Data storage

| What | Where | Protection |
|---|---|---|
| Activity sessions, captured frames, OCR and accessibility text | `~/.meridian/meridian.db` | **Encrypted at rest** (SQLCipher). The key is generated on first run and held in your OS keychain - macOS Keychain or Windows Credential Manager. |
| OAuth tokens (Jira, GitHub, Trello) | `~/.meridian/oauth/<provider>.json` | File mode `0600` - readable only by your user account. |
| API keys and personal access tokens (Linear, Azure DevOps, and the non-OAuth options for Jira and GitHub) | `~/.meridian/.env` | File mode `0600`. |
| Logs and traces | `~/.meridian/telemetry/` | Local only, full fidelity. Read them with `meridian logs`. |
| Crash safety-net logs | `~/.meridian/logs/` | Raw stdout/stderr captured by the OS service manager, size-capped. |

Raw captured frames are pruned automatically after 30 days once the ETL pipeline has consumed them (`MERIDIAN_CAPTURE_RETENTION_DAYS`).

**You own all of this.** Delete it, export it, or migrate it at any time.

---

## Error reporting

**This is the one thing Meridian sends to Meridiana, and it is on by default in packaged installs.** You can turn it off at any time in **Settings → Capture & Privacy → Error reporting**. Turning it off stops both paths described below immediately.

Builds you compile from source never ship anything — the endpoint and credentials are injected only at release time, so a source build is inert regardless of this setting.

### What is sent

Only records that represent a problem:

- log records at **WARN and above**, and
- trace spans whose status is **ERROR**.

Everything else — the high-volume `INFO`/`DEBUG` records, and every successful span — is dropped before the network leg. Those are the records that carry content, so they never reach the wire at all.

For the records that do qualify, every attribute is filtered on your device before sending:

- **Numbers and booleans** are kept — they are counts, durations, token totals, status codes, and structurally cannot hold your content.
- **Text values are dropped unless the attribute name is on an explicit allowlist.** Anything path-like is scrubbed of your home directory; the small free-text subset (error messages, stack traces) is additionally scrubbed of URLs, email addresses, and token-shaped strings, then length-clamped.
- **Structured values** (byte blobs, arrays, nested maps) are dropped outright.
- **Span events and links are cleared entirely.**

The filter fails closed: a newly added attribute anywhere in the codebase is dropped by default until someone deliberately allowlists it.

### What is therefore never sent

OCR text, accessibility-tree content, window titles, browser URLs, coding-agent conversation bodies, LLM prompts and completions, ticket contents, file paths, and your local database. These stay on your machine even when error reporting is on. Local logs remain full-fidelity for your own debugging — the stripping applies only to the copy that would be transmitted.

### How reports are identified

An error report carries a **Support ID** instead of your hostname. It is a one-way pseudonym, and it exists so a support ticket you file can be matched to the errors your machine actually reported. It is derived from a hardware identifier, not your hostname — on macOS the hostname is routinely derived from your network and often contains your real name.

Nothing in an error report is associated with an account, and the hash cannot be reversed. You can see your own Support ID in **Settings → Account**.

> **Alpha exception, until 28 August 2026.** While Meridian is in invite-only alpha, signing in changes this: the Support ID is instead derived from a salted hash of your account email, so support can follow one tester's errors across their machines. Your raw email address is never sent. Settings → Account states which mode is currently active for you. After that date every install reverts to the hardware-derived pseudonym automatically.

### Crash reports

Packaged builds also send crash reports (via Sentry) so we learn about crashes that happen before anything can be logged. These carry the same Support ID pseudonym in place of your hostname, and are governed by the **same** Error reporting switch — turning it off disables crash reporting too.

### Sending diagnostics manually

**Settings → Account → Export Diagnostics** (or `meridian telemetry export`) bundles your local logs into a `.tar.gz` you can inspect and send to support by hand. This is independent of the switch above, it only happens when you ask for it, and it contains nothing the automatic path would not already send.

---

## Third-party integrations

Meridian can connect to **Jira**, **GitHub**, **Linear**, **Trello**, and **Azure DevOps** to post worklogs and ticket updates.

When you authorize an integration:

1. You log into the third-party service — your browser, your credentials.
2. The OAuth token or API key is stored **only on your machine** (see [Data storage](#data-storage)).
3. Meridian calls the service's API **directly from your machine** using that token.
4. Meridiana never sees your credentials, your tokens, or the data you exchange.

**Nothing posts without your approval.** Meridian drafts worklogs and ticket updates; you review and approve each one in the dashboard. Approval is the only gate.

---

## LLM provider (summarisation & ticket matching)

Capture, OCR, and session structuring all run **on-device**. The AI that writes worklog summaries and matches your work to the right ticket runs through the coding-agent CLI you already use — Claude Code, Codex, Cursor, or Copilot CLI, on your own account — or a cloud LLM endpoint you configure (any OpenAI-compatible endpoint, e.g. OpenRouter).

There is no on-device generative model. The only model that runs fully locally is a small text-embedding model used to de-duplicate similar activity before summarising; it never sends anything off your machine.

**Because this step uses an LLM, session text — which may include OCR'd screen content — is sent to whichever provider you choose** so it can produce the summary. Choose a provider you trust and review its data-handling policy. Meridiana is never in this path: the request goes directly from your machine to your configured provider.

---

## Accounts

Signing in is optional and is currently used to gate the invite-only alpha. Your email address is held by our authentication provider (Clerk) and is never attached to error reports — see the alpha note under [How reports are identified](#how-reports-are-identified) for the one way sign-in affects reporting.

---

## Your rights

- **Access** — your data is in SQLite on your own disk; inspect or export it any time.
- **Delete** — `meridian uninstall` removes Meridian's local data and services. Deleting `~/.meridian/` by hand does the same for data alone.
- **Portability** — export your activity data and switch tools; there is no lock-in.
- **Opt out** — one switch at Settings → Capture & Privacy → Error reporting stops everything Meridiana would otherwise receive.
- **No tracking** — Meridian does not track you across devices, sites, or sessions, and there are no analytics or usage-metrics servers.

---

## Contact

For privacy questions or concerns, email **akarsh@meridiona.com**.

---

## Changes to this policy

We may update this policy from time to time. Material changes are made in this file, in a public repository, so the history is auditable.
