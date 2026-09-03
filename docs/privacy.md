//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

# Privacy Policy

**Last updated:** August 2026

## Overview

Meridian is a local-first developer tool that turns your activity into structured work sessions and keeps your project management in sync. Screen capture, OCR, and session structuring run entirely on your machine, and what they produce is stored in an encrypted local database. Meridiana never receives your screen content, your window titles, or the text captured from your screen.

Meridian does make network calls. There are **five kinds**, listed here in full:

1. **Ticket updates you approve** — sent directly from your machine to the trackers you connect.
2. **AI summarisation** — session text, including text captured from your screen, sent to the LLM provider you choose. **This is the one path on which your captured content leaves the device**, and it goes to a provider you pick and configure, never to Meridiana. It is described in [LLM provider](#llm-provider-summarisation--ticket-matching).
3. **Error reports** — redacted, error-only diagnostics sent to Meridiana. **On by default in packaged installs; one switch turns it off.**
4. **Product analytics** — three events, and only once you sign in: that you installed, that you were active on a given day, and a daily count of what Meridian did for you. **No screen content. On by default; one switch turns it off** — see [Product analytics](#product-analytics).
5. **A public counter ping** — one anonymous "+1" to meridiona.com each time you post a worklog **or update a personal task**, powering the counter on the landing page. No payload. Sent only from release builds, and it has **no off switch**.

**How much of this you control:** 1 and 2 happen only because you connected a tracker
and chose a provider; 3 and 4 each have their own switch in Settings, and they are
separate switches on purpose — turning off crash reports should not silently stop
usage reporting, or the reverse. 5 does not have one. Saying "you control all of
them" would be simpler and would not be true.

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

Outside the alpha window described below, nothing *inside* an error report is associated with an account, and the hash cannot be reversed. You can see your own Support ID in **Settings → Account**, which also states which mode is currently active for you.

> **But if product analytics is on, we can still connect the two.** Product-analytics events carry both your account email and the same Support ID, so the pair links your errors to you. Outside the alpha window below, that linkage lives entirely on the analytics side - an error report itself does not contain your email. Turning off **Product analytics** removes it; see [Product analytics](#product-analytics).
>
> **Alpha exception, while you are signed in, until 31 December 2026.** While Meridian is in invite-only alpha, signing in changes the above in two ways: the Support ID is derived from a salted hash of your account email instead of a hardware identifier, so support can follow one tester's errors across their machines; and, in addition, **your actual account email is attached directly to error reports and crash reports** (as a plain field alongside the Support ID) so support can identify which signed-in tester and which machine an issue came from without a separate lookup step. For that period, the two statements above about an error report not containing or being associated with your account do not hold. Settings → Account states which mode is currently active for you.
>
> The exception ends at **00:00 UTC on 1 January 2027** - so 31 December 2026 is the last day it applies. Every install then reverts automatically, with no update required: the Support ID goes back to the hardware-derived pseudonym, your email stops being attached to reports, and any copy still stored on your machine is removed the next time Meridian starts. Signing out ends it for you at any time.

### Crash reports

Packaged builds also send crash reports (via Sentry) so we learn about crashes that happen before anything can be logged. These carry the same Support ID pseudonym in place of your hostname, and are governed by the **same** Error reporting switch — turning it off disables crash reporting too. During the alpha window described above, a crash report also carries your account email for a signed-in tester, exactly like an error report does.

### Sending diagnostics manually

**Settings → Account → Export Diagnostics** (or `meridian telemetry export`) bundles your local logs into a `.tar.gz` you can inspect and send to support by hand. It is independent of the switch above and only happens when you ask for it.

**Treat this archive as more sensitive than an automatic error report, and look inside before you send it.** The two are not the same content. Automatic reports are filtered to warnings and errors and are redacted on your machine before they leave; the export is a copy of your **raw local spool**, which is captured at full fidelity. That means it can contain `INFO` and `DEBUG` records, and diagnostic detail, that the automatic path deliberately strips or never sends. It is a `.tar.gz` of ordinary files - `meridian logs` renders the same data if you would rather read it that way first.

---

## Product analytics

Meridian sends three product-analytics events to PostHog Cloud, so we can see how many
people install it, whether they keep using it, and whether it is actually working for
them.

**Nothing is sent until you sign in.** Signed out, this path is entirely inactive.

**You can turn it off:** Settings → Capture & Privacy → **Product analytics**. It is on
by default. This is a *separate* switch from error reporting, because the two send
different things about you - see [Error reporting](#error-reporting).

| Event | When | What it contains |
|---|---|---|
| `app_installed` | Once per device, per signed-in account | Nothing beyond the shared properties below. |
| `app_active` | Once per calendar day Meridian is running | The shared properties, plus the current-state summary described below (which AI provider and which trackers). It exists so we can tell how many people come back on a given day, and so a brand-new install's setup is visible without waiting for the first `daily_usage`. |
| `daily_usage` | Once per completed calendar day | The categories listed below. |

`daily_usage` carries:

- **Your day in four numbers** - focus hours, coding hours, logged hours, and number of
  drafts. The same four the dashboard's Today card shows you.
- **What Meridian did for you** - counts only: how many tickets it updated and on which
  trackers, how many worklog posts failed, whether you confirmed or skipped the daily
  plan and how many items were in it, how many day-tasks it produced and how many you
  corrected, whether a day summary was written, and how many notifications were sent,
  delivered, or failed to deliver.
- **Whether your install is healthy** - is the daemon running, is the database readable,
  and which internal warnings are active (as short internal codes such as `db.corrupt`,
  never their text).
- **Which AI provider you use, and whether it works** - the provider's name from our own
  fixed list (`claude`, `codex`, `cursor`, `copilot`, `custom`), whether it is currently
  usable or rate-limited, whether you actually chose that provider or are still on the
  default, and for a cloud endpoint the vendor it came from (one of `groq`, `openai`,
  `gemini`, `openrouter`, `other`) and the model id. If you entered your own endpoint by
  hand, the **model id is not sent** - it can name an internal deployment. A vendor value
  we don't recognise is replaced with `unrecognised` rather than sent as-is. Your
  endpoint URL and API key are never sent under any setting.
- **Which trackers you connect, and whether they sync** - the tracker's name from our
  fixed list (`jira`, `linear`, `github`, `trello`, `azure_devops`) and a one-word status
  for each: syncing fine, stale, failing, never synced, or waiting for you to pick a
  project. This is the one thing here that describes your workplace rather than Meridian,
  and it is the name only.
- **How much CPU and memory Meridian itself used** - and nothing about any other program
  on your machine. Once a minute Meridian measures two processes by their process id: the
  menu-bar app and its background service. It records the highest and average of each
  over the day, along with how many measurements were taken. It does **not** list, count,
  or measure your other applications, and it does not measure the AI command-line tools -
  those are programs you run yourself, and telling them apart from ours reliably is not
  possible, so they are left out rather than guessed at. This exists so we can tell
  whether a release made Meridian heavier on real machines instead of only on ours.

All three carry: your app version, your OS name (`macos` / `windows`), the release
channel, a random per-device UUID, and your Support ID. Location lookup is explicitly
disabled on every event.

**These events identify you by your account email.** It is used directly as the PostHog
`distinct_id`, which is why nothing is sent before sign-in. This path is identified, not
pseudonymous, and we would rather say so plainly than bury it.

**A small current-state record is kept against your account**, updated on each of the
events above rather than accumulating one row per day: your email, app version, release
channel, OS, which AI provider and model you are on, and which trackers you have
connected. It exists so we can answer "what is this user's setup" without replaying
their history. It is a subset of what the events above already carry - nothing is
collected for it that is not listed here - and it is overwritten each time, so it always
reflects your current setup rather than a trail. Your Support ID is deliberately kept
out of it.

**And they carry your Support ID, which links this to your error reports.** Error
reports and crash reports are pseudonymous on their own - they identify a machine, not
a person. Including the same Support ID here means that, for as long as product
analytics is on, we can connect the two. We do this so that when something breaks we
can tell *who* it is broken for and reach out, rather than staring at an anonymous
error we cannot act on. If you would rather we could not make that connection, turning
off product analytics breaks it: error reporting alone never carries your email.

**What it never contains:** screen content, OCR text, window titles, browser URLs,
application names, ticket keys, ticket titles or contents, notification text, summary
text, file paths, or anything about *what* you worked on. Nor anything that names your
employer: your Jira or Azure DevOps instance URL, project keys, board or team ids,
workspace names, your AI endpoint's URL or API key, or the text of any tracker sync
error. Every value above is a count, a yes/no, a date, an id from a list we defined in
advance, or one of the identifiers named earlier in this section - your account email,
a random per-device UUID, your Support ID, your app version, and, for a known cloud
endpoint, its model id. They describe what Meridian did, or who is using it, not what
you did or who you work for.

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

Signing in is required to use Meridian - it is a one-time step, not an account with a password: you verify a code sent to your email, delivered via AWS SES and checked through a Cloudflare Worker we operate. There is no invite-only alpha it gates. During the alpha window described below, your email is also attached directly to your error and crash reports so support can identify which tester and machine an issue came from — see the alpha note under [How reports are identified](#how-reports-are-identified) for the full detail and its end date.

---

## Your rights

- **Access** — your data is in SQLite on your own disk; inspect or export it any time.
- **Delete** — `meridian uninstall` removes Meridian's local data and services. Deleting `~/.meridian/` by hand does the same for data alone.
- **Portability** — export your activity data and switch tools; there is no lock-in.
- **Opt out of error reporting or product analytics** — each has its own switch at Settings → Capture & Privacy, and they can be turned off independently. Staying signed out disables product analytics entirely regardless of the switch.
- **No behavioural tracking** — Meridian does not follow you across websites, does not record sessions, and never sells your data or shares it for advertising. It does reach the service providers named throughout this document to do the job each is described for - PostHog for product analytics, Sentry for crash reports, AWS SES and a Cloudflare Worker we operate for sign-in - and nowhere else. The only usage data collected is the three events described under [Product analytics](#product-analytics).

---

## Contact

For privacy questions or concerns, email **akarsh@meridiona.com**.

---

## Changes to this policy

We may update this policy from time to time. Material changes are made in this file, in a public repository, so the history is auditable.
