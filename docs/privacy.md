//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

# Privacy Policy

**Last updated:** August 2026

## Overview

Meridian is a local-first developer tool that turns your activity into structured work sessions and keeps your project management in sync. Screen capture, OCR, and session structuring run entirely on your machine, and what they produce is stored in an encrypted local database. Meridiana never receives your screen content, your window titles, or the text captured from your screen.

Meridian does make network calls. There are **five kinds**, listed here in full:

1. **Ticket updates you approve** — sent directly from your machine to the trackers you connect.
2. **AI summarisation** — session text, including text captured from your screen, sent to the LLM provider you choose. **This is the one path on which your captured content leaves the device**, and it goes to a provider you pick and configure, never to Meridiana. It is described in [LLM provider](#llm-provider-summarisation--ticket-matching).
3. **Error reports** — redacted, error-only diagnostics sent to Meridiana. **On by default in packaged installs; one switch turns it off.**
4. **Product analytics** — two events, and only once you sign in: that you installed, and a daily count of hours and drafts. **No screen content. Currently no off switch** — see [Product analytics](#product-analytics).
5. **A public counter ping** — one anonymous "+1" to meridiona.com each time you post a worklog **or update a personal task**, powering the counter on the landing page. No payload. Sent only from release builds, and it has **no off switch** either.

**How much of this you control:** 1 and 2 happen only because you connected a tracker
and chose a provider; 3 has a switch in Settings. 4 and 5 do not currently have one.
Saying "you control all of them" would be simpler and would not be true.

Each is described in full below. If you only read two sections, read
[Error reporting](#error-reporting) and [Product analytics](#product-analytics).

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

> **Alpha exception, until 28 August 2026.** While Meridian is in invite-only alpha, signing in changes this in two ways: the Support ID is derived from a salted hash of your account email instead of a hardware identifier, so support can follow one tester's errors across their machines; and, in addition, your actual account email is attached directly to error reports and crash reports (as a plain field alongside the Support ID) so support can identify which signed-in tester and which machine an issue came from without a separate lookup step. Settings → Account states which mode is currently active for you. After that date every install reverts automatically: the Support ID goes back to the hardware-derived pseudonym, and your email stops being attached to reports, with no update required.

### Crash reports

Packaged builds also send crash reports (via Sentry) so we learn about crashes that happen before anything can be logged. These carry the same Support ID pseudonym in place of your hostname, and are governed by the **same** Error reporting switch — turning it off disables crash reporting too. During the alpha window described above, a crash report also carries your account email for a signed-in tester, exactly like an error report does.

### Sending diagnostics manually

**Settings → Account → Export Diagnostics** (or `meridian telemetry export`) bundles your local logs into a `.tar.gz` you can inspect and send to support by hand. It is independent of the switch above and only happens when you ask for it.

**Treat this archive as more sensitive than an automatic error report, and look inside before you send it.** The two are not the same content. Automatic reports are filtered to warnings and errors and are redacted on your machine before they leave; the export is a copy of your **raw local spool**, which is captured at full fidelity. That means it can contain `INFO` and `DEBUG` records, and diagnostic detail, that the automatic path deliberately strips or never sends. It is a `.tar.gz` of ordinary files - `meridian logs` renders the same data if you would rather read it that way first.

---

## Product analytics

Meridian sends two product-analytics events to PostHog Cloud, so we can see how many
people install it and whether they keep using it.

**Nothing is sent until you sign in.** Signed out, this path is entirely inactive.

| Event | When | What it contains |
|---|---|---|
| `app_installed` | Once per device, per signed-in account | Nothing beyond the shared properties below. |
| `daily_usage` | Once per completed calendar day | Focus hours, coding hours, logged hours, and number of drafts - the same four numbers the dashboard's Today card shows you. |

Both carry: your app version, your OS name (`macos` / `windows`), the release channel,
and a random per-device UUID. Location lookup is explicitly disabled on every event.

**These events identify you by your account email.** It is used directly as the PostHog
`distinct_id`, which is why nothing is sent before sign-in. This is a real trade-off and
we would rather state it plainly than bury it: this path is identified, not
pseudonymous, and unlike error reporting it **does not currently have an off switch**.
An opt-out is planned - if you want it sooner, say so on
[the issue tracker](https://github.com/Meridiona/meridian/issues) and it will move up.

**What it never contains:** screen content, OCR text, window titles, browser URLs,
application names, ticket keys, ticket contents, file paths, or anything about *what*
you worked on. The four daily numbers are totals, not a description of your day.

Analytics are captured through a single plain HTTPS request. Meridian does not embed
PostHog's browser SDK, so session replay, autocapture, surveys, and feature flags are
never active.

### The public counter

When you post a worklog or update a personal task, Meridian sends a single anonymous
"+1" to `meridiona.com/api/counter/increment`. It carries **no payload** - not who,
not what, not when beyond the moment of the request. It exists only to increment the
"updates logged and counting" number on the landing page, and it is sent only from
release builds.

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

Signing in is optional and is currently used to gate the invite-only alpha. Your email address is held by our authentication provider (Clerk); during the alpha window it is also attached directly to your error and crash reports so support can identify which tester and machine an issue came from — see the alpha note under [How reports are identified](#how-reports-are-identified) for the full detail and its end date.

---

## Your rights

- **Access** — your data is in SQLite on your own disk; inspect or export it any time.
- **Delete** — `meridian uninstall` removes Meridian's local data and services. Deleting `~/.meridian/` by hand does the same for data alone.
- **Portability** — export your activity data and switch tools; there is no lock-in.
- **Opt out of error reporting** — one switch at Settings → Capture & Privacy. Product analytics does not yet have an equivalent switch; staying signed out disables it entirely.
- **No behavioural tracking** — Meridian does not follow you across websites, does not record sessions, and sells or shares nothing with anyone. The only usage data collected is the two events described under [Product analytics](#product-analytics).

---

## Contact

For privacy questions or concerns, email **akarsh@meridiona.com**.

---

## Changes to this policy

We may update this policy from time to time. Material changes are made in this file, in a public repository, so the history is auditable.
