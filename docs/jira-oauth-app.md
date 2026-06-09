# Atlassian OAuth app (maintainer runbook)

Meridian connects Jira via **browser OAuth** (Authorization Code + PKCE). This
needs **one public Atlassian OAuth 2.0 (3LO) app**, registered **once by
Meridiona**. End users never touch any of this — they run `meridian oauth-login
jira` (or the installer does it for them) and click **Accept** in their browser.

The client id lives in `src/intelligence/oauth/jira.rs` → `DEFAULT_CLIENT_ID`.

**Atlassian Cloud also requires a confidential `client_secret`** at the token
endpoint, even for desktop apps: it accepts the PKCE `code_challenge` at
`/authorize` but **ignores PKCE at the token step** and rejects a secret-less
exchange with `access_denied` / `Unauthorized` (a known, still-open limitation —
[OAUTH20-2491](https://jira.atlassian.com/browse/OAUTH20-2491)). So PKCE alone is
**not** enough here.

The secret is **never committed to source.** It is provided to the release build
from the **`JIRA_OAUTH_CLIENT_SECRET`** GitHub Actions secret (wired in
`.github/workflows/release.yml`). A plain source build doesn't have it, so a
source-built daemon must supply `JIRA_OAUTH_CLIENT_SECRET` at runtime, or use the
API-token fallback.

---

## One-time registration

1. **Create the app** — https://developer.atlassian.com/console/myapps/ →
   **Create** → **OAuth 2.0 integration**. Name it "Meridian".
   - Own it under a **Meridiona-controlled Atlassian account**, not a personal
     one, so it doesn't get orphaned when someone leaves.

2. **Permissions** → add the **Jira API**, then add these **classic** scopes:
   | Scope | Why |
   |---|---|
   | `read:jira-work` | fetch the user's assigned issues |
   | `write:jira-work` | post worklogs / comments |
   | `read:jira-user` | the `/myself` probe in `meridian doctor` |

   > `offline_access` is **not** a console scope — Meridian requests it at runtime
   > to get a refresh token. Don't look for a checkbox; there isn't one.

3. **Authorization** → OAuth 2.0 (3LO) → **Callback URL** (exact match):
   ```
   http://127.0.0.1:9123/callback
   ```
   Use the **IP, not `localhost`** (the console greys out Save for `localhost`,
   and the IP avoids a `localhost`→`::1` loopback-bind mismatch). This must
   byte-match the client's redirect — port is `JIRA_OAUTH_REDIRECT_PORT`,
   default `9123`.

4. **Distribution → Enable sharing** (make the app **Distributable**). ⚠️
   **REQUIRED before any non-Meridiona user can connect.** A private (non-shared)
   3LO app can only be authorized by users **in the development org**. External
   customers hit a *"your site admin must authorize this app"* / blocked error
   until sharing is on. This is a console toggle — no code change.

5. **Settings → Authentication details** → copy the **Client ID** and put it in
   `DEFAULT_CLIENT_ID` (`src/intelligence/oauth/jira.rs`).

6. **Settings → Authentication details** → copy the **Secret** and store it as the
   **`JIRA_OAUTH_CLIENT_SECRET`** GitHub Actions secret (repo `Meridiona/meridian`
   → Settings → Secrets and variables → Actions). The release workflow uses it at
   build time. **Without this, OAuth login fails for everyone.**
   - **Rotation:** regenerate the secret in the console only when compromised, then
     update the Actions secret and **cut a release** — released clients pick up the
     new secret only via a new release, so they keep working on the old one until
     then. Do **not** rotate on a schedule (it would break every installed client).
     See `KAN-159`.

---

## What each end user gets

- They connect to **their own** Jira site — discovered from who signs in (the
  `accessible-resources` endpoint), never hardcoded.
- Tokens are stored at `~/.meridian/oauth/jira.json` (mode `0600`) and
  auto-refreshed (Atlassian access tokens last ~1 h and rotate the refresh
  token; Meridian persists each rotation).
- An install-time override is available: set `JIRA_OAUTH_CLIENT_ID` to point at a
  different app (e.g. a customer's own app, or Jira Data Center).

## Fallback: orgs that block third-party apps

Some Atlassian orgs require admin approval for OAuth apps, or disable end-user app
installs entirely. There, the browser login fails and the user falls back to a
**static API token** (`JIRA_BASE_URL` / `JIRA_EMAIL` / `JIRA_API_TOKEN`) — which
needs no org-level app approval. The installer offers this automatically on
decline or failure, and the `oauth-login` failure message points at it.

---

## Pre-GA checklist

- [ ] App created under a Meridiona-owned account
- [ ] Scopes: `read:jira-work`, `write:jira-work`, `read:jira-user`
- [ ] Callback `http://127.0.0.1:9123/callback`
- [ ] **Distribution → Enable sharing (Distributable)** ← easy to forget; gates all external users
- [ ] Client ID in `DEFAULT_CLIENT_ID`
- [ ] **`JIRA_OAUTH_CLIENT_SECRET` set as a GitHub Actions secret** ← OAuth login fails for everyone without it
