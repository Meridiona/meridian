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

- **No telemetry or analytics servers** — nothing phones home to Meridiana
- **Capture and classification are on-device** — screen content does not leave the machine unless you configure an optional cloud LLM
- **OAuth tokens** are stored in `~/.meridian/oauth/` at mode `0600`; API keys go in `~/.meridian/.env` with the same restriction
- **The only outbound network calls** are approved ticket updates sent directly from your machine to the trackers you connect (Jira, GitHub, Linear) — documented in [docs/privacy.md](docs/privacy.md)
- **The screenpipe database is opened read-only** — Meridian holds no write lock on it
