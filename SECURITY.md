# Security Policy

## Scope

Meridian processes sensitive data — screen content, OCR text, accessibility tree metadata, and authentication tokens for your project management tools. We take security seriously.

This policy covers the Meridian daemon, dashboard, MCP server, and tray app in this repository.

## Supported versions

We support the latest published release. Security fixes are applied to the main branch and included in the next release.

| Version | Supported |
|---|---|
| Latest (`main`) | ✅ |
| Older releases | ❌ patch on request |

## Reporting a vulnerability

**Do not report security issues in public GitHub Issues.**

Please report privately by emailing **akarsh@meridiona.com** with:

- A description of the issue and the potential impact
- Steps to reproduce (or a proof-of-concept, if applicable)
- The affected component (daemon, dashboard, MCP server, tray, installer)
- Your contact details for follow-up

We will acknowledge receipt within **48 hours** and aim to resolve critical issues within **14 days**. We'll keep you informed throughout and credit researchers who responsibly disclose valid vulnerabilities.

## What we consider in scope

- **Local privilege escalation** — anything that lets a process or user escalate beyond what Meridian requires
- **Credential leakage** — OAuth tokens, API keys, or screen content being written to unintended locations, logged, or transmitted unexpectedly
- **Unintended data exfiltration** — any network calls not documented in the privacy policy and not explicitly initiated by the user
- **Installer / bootstrap script** — supply-chain issues in `scripts/bootstrap.sh` or `install.sh`
- **MCP server** — injections or privilege issues when Meridian's MCP tools are called from an AI client

## What we consider out of scope

- Issues that require physical access to the machine
- Theoretical attacks with no realistic exploitation path
- Social engineering of users

## Security architecture notes

Meridian is designed to contain blast radius by default:

- **Capture and classification are on-device** — screen content does not leave the machine except via the LLM provider you configure for summarisation
- **The activity database is encrypted at rest** — `~/.meridian/meridian.db` uses SQLCipher, with the key generated on first run and held in the OS keychain (macOS Keychain / Windows Credential Manager)
- **Credentials are stored locally with restrictive permissions** — OAuth tokens in `~/.meridian/oauth/<provider>.json` and tracker credentials in `~/.meridian/.env`, both at mode `0600`. One known exception is tracked publicly: the Clerk session token is persisted as plaintext JSON by the upstream auth plugin ([#727](https://github.com/Meridiona/meridian/issues/727))
- **Product analytics are minimal and content-free** — two events (an install, and a daily count of hours and drafts), sent only after sign-in and identified by account email. No screen content, application names, ticket data, or file paths. A separate payload-free counter ping fires per posted worklog. Detail in [docs/privacy.md](docs/privacy.md#product-analytics)
- **Error reporting is redacted, error-only, and switchable** — packaged builds send WARN-and-above logs and ERROR-status spans to Meridiana's own observability backend, on by default, with an off switch at Settings → Capture & Privacy. Content-bearing attributes are stripped on-device before the network leg and the hostname is replaced by a one-way pseudonym; source builds never ship at all. Full detail in [docs/privacy.md](docs/privacy.md#error-reporting)
- **Approved ticket updates go directly from your machine** to the trackers you connect (Jira, GitHub, Linear, Trello, Azure DevOps) — never proxied through Meridiana
