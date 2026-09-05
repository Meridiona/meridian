# otp-worker

Cloudflare Worker backing Meridian's one-time email+OTP capture step (the
setup wizard's replacement for Clerk — see the parent plan,
`giggly-jumping-hopcroft.md`, for the full "why"). Sends a 6-digit code to an
email address via AWS SES and verifies it. No accounts, no sessions, no
sign-out — ask once, verify once, store the email locally, never re-check.

This is the **first live Cloudflare Worker in this repo.** Read "Why this
design" below before changing anything auth- or rate-limit-related — a prior
Worker (`infra/hf-proxy`, since deleted) shipped unauthenticated with no rate
limit, got hammered for 173,088 requests in a day against a 100k/day
account-wide cap, and took `meridiona.com` down. CLAUDE.md's Hard Rules
section has the full incident writeup; this Worker exists specifically not to
repeat it.

## Routes

Exactly two exist. Everything else — wrong path, wrong method — gets a plain
404. Both require `Authorization: Bearer <token>`.

| Route | Body | Purpose |
|---|---|---|
| `POST /otp/send` | `{ email, turnstileToken? }` | Generate a code, email it via SES |
| `POST /otp/verify` | `{ email, code, previousEmail? }` | Check a code against the live record |

`previousEmail` is optional and purely informational — the client's best
knowledge of the address it had on file before this verify, used only to
decide what (if anything) to tell `NOTIFY_EMAIL` about (see "Account-event
notification" below). It is never used for any security decision.

### Status codes

The plan specified the set of codes (400/401/403/410/429/503) but not the
full route-by-route mapping; this is what was actually implemented, and each
distinct code is meant to map to a distinct client-side message:

**`/otp/send`**

| Status | Body | Meaning |
|---|---|---|
| 200 | `{ ok: true }` (or `{ ok: true, code }` — staging only, see below) | Email queued for delivery |
| 400 | `{ error: "invalid_email" \| "invalid_json" \| "invalid_turnstile_token" }` | Malformed request |
| 401 | `{ error: "unauthorized" }` | Missing/wrong bearer token |
| 403 | `{ error: "turnstile_failed" }` | A `turnstileToken` was sent and failed verification |
| 429 | `{ error: "rate_limited", scope: "email" \| "ip" \| "global" }` | One of the three send caps tripped |
| 503 | `{ error: "email_delivery_failed" }` | SES call failed after all gates passed |

**`/otp/verify`**

| Status | Body | Meaning |
|---|---|---|
| 200 | `{ ok: true, verified: true }` | Code matched; record consumed |
| 200 | `{ ok: true, verified: false, attemptsRemaining }` | Wrong code, record still live — not an error, the caller can retry |
| 400 | `{ error: "invalid_email" \| "invalid_json" \| "invalid_code" }` | Malformed request |
| 401 | `{ error: "unauthorized" }` | Missing/wrong bearer token |
| 410 | `{ error: "code_expired_or_not_found" }` | No live record — never sent, naturally expired, or attempts just exhausted |

`exhausted` (5th wrong guess) and `not_found_or_expired` (no record at all)
are deliberately collapsed into the same 410 — a caller must not be able to
tell "you used up your attempts" from "there was never a code for this
email" from the HTTP layer; both just mean "request a new code."

## Why this design (hf-proxy postmortem, applied)

Four things this Worker does that `infra/hf-proxy` didn't, each mapped to a
line item in CLAUDE.md's Hard Rules:

1. **Authenticates every request.** `auth.ts` checks `Authorization: Bearer`
   before any body parsing or KV access, on both routes, with no
   unauthenticated path. Empty/unconfigured secrets never match (guards the
   "both sides blank" bypass).
2. **Allowlists the paths it serves.** The router in `index.ts` is an
   exhaustive `if/if/else 404` — there is no default-allow branch.
3. **Rate-limits.** Three independent KV-backed caps (per-email, per-IP,
   global-daily) gate every send, checked before any SES call is attempted.
4. **Has exactly one caller** (the Meridian tray) and a name that says so.
   When the tray stops calling this, delete it — don't leave it running with
   a live DNS record, the way hf-proxy did after the MLX stack that used it
   was removed. Cloudflare publishes every hostname to Certificate
   Transparency logs the moment it issues a cert, so an unused endpoint is
   discoverable whether or not it's advertised.

Bearer-auth honesty note: the token is compiled into the shipped tray binary
(mirrors `tray/src-tauri/src/counter_ping.rs`'s `DEFAULT_COUNTER_API_KEY`
pattern) and is therefore extractable by anyone with the binary. It proves
"a genuine Meridian build sent this," **not** "a human is present." Rate
limiting is the actual abuse containment; the bearer token only keeps out
callers who never had a Meridian binary in the first place.

## KV schema (`OTP_KV`)

Keys are built from `sha256(normalizeEmail(email))` — the raw email is never
used as a KV key. `rl:ip:<ip>` is the one exception, keyed on the literal
`CF-Connecting-IP` value, per the plan.

| Key | Value | Notes |
|---|---|---|
| `code:<hash>` | `{ codeHash, attempts, expiresAt }` | `codeHash` is `HMAC-SHA256(OTP_CODE_PEPPER, code)` — never the bare code |
| `rl:email:<hash>` | `{ count, expiresAt }` | Rolling 24h window from first send, cap `RL_EMAIL_PER_DAY` |
| `rl:ip:<ip>` | `{ count, expiresAt }` | Rolling 1h window, cap `RL_IP_PER_HOUR` |
| `global:sends:<UTC date>` | `{ count, expiresAt }` | One key per calendar day, cap `RL_GLOBAL_PER_DAY`, cost/abuse containment |

Every record embeds its own authoritative `expiresAt` (epoch ms) rather than
relying solely on KV's own TTL — two independent reasons, both documented in
`kv.ts`/`otp.ts`/`ratelimit.ts`:

- A rolling window's expiry must survive a read-modify-write without being
  reset on every increment, and Cloudflare KV requires `expirationTtl` to be
  re-specified on **every** `put` (omitting it clears the expiry entirely).
- **Cloudflare Workers KV enforces a hard minimum `expirationTtl` of 60
  seconds.** A code with 20 real seconds left cannot be written back with a
  20-second KV TTL. `kv.ts`'s `ttlSecondsFromExpiry` clamps to that floor —
  this only affects the KV-level storage-cleanup timer; the embedded
  `expiresAt` check in `otp.ts`/`ratelimit.ts` is what actually enforces
  expiry, so a clamped KV TTL can never let an expired record be honoured. It
  can only make a dead key linger in storage slightly longer before KV itself
  reaps it.

Counters are persisted **before** the SES call is attempted, not after — the
caps exist for cost/abuse containment against attempted sends, so a run of
SES failures (an outage, a bad credential) still counts against budget.
Otherwise, an attacker (or an outage) could drive unlimited send-attempt
traffic for free by ensuring every attempt "fails" cheaply.

## Code hashing

`OTP_CODE_PEPPER` (a Worker secret) is mixed into every code via
HMAC-SHA256 before it touches KV — never a bare hash, since a 6-digit code is
trivially brute-forced offline from a raw KV dump otherwise. Verify caps
attempts at `MAX_VERIFY_ATTEMPTS` (5) before invalidating the code and
forcing a fresh send. **A wrong guess never extends the record's remaining
TTL** — `otp.ts`'s `verifyOtpAttempt` carries the original `expiresAt`
forward unchanged on every wrong guess; this is pinned by
`otp.test.ts`'s "never extends the TTL" cases.

## Email delivery (AWS SES)

`ses.ts` calls SES's `SendEmail` action (Query API, `2010-12-01`),
SigV4-signed via [`aws4fetch`](https://github.com/mhart/aws4fetch) —
Workers run on a V8 isolate with no Node.js APIs, so the official AWS SDK
doesn't work here; `aws4fetch` is the established community pattern for
calling AWS from a Worker.

**Resolved, not specified by the plan: SES's v1 Query API over SESv2's JSON
API.** Picked because it's the one with a documented, minimal `aws4fetch`
example (`service: "email"`, form-urlencoded body) — a single `SendEmail`
call is low-stakes either way and this was the path of least friction.

`from` is `${FROM_NAME} <${FROM_ADDRESS}>` — currently
`Meridian <otp@auth.meridiona.com>`, a placeholder verified subdomain of
`meridiona.com` matching the existing `telemetry.`/`observe.` subdomain
convention. **DNS verification (SPF/DKIM records in SES, added to the
existing Cloudflare-managed zone) is a manual step outside this Worker's
code** — see "Manual steps before first deploy" below.

The email body is plain text, no links: "Your Meridian verification code is:
`<code>`. This code expires in `<N>` minutes...".

SES error responses are never logged verbatim (`extractSesErrorCode`) —
SES's sandbox-mode "email address is not verified" error echoes the
destination address back in the response text, which must never reach
Workers Logs.

## Account-event notification

On a successful `/otp/verify` (the `verified` case only — never on a wrong
guess), `handleVerify` fires a second, unrelated SES send to `NOTIFY_EMAIL`
(`vars.NOTIFY_EMAIL` in `wrangler.jsonc`, currently `company@meridiona.com` on
both channels) telling the company that an install signed up or changed its
email. This rides on `ctx.waitUntil` exactly like the rate-limit alert below —
fire-and-forget, never awaited inline, so a failed notification send can
never affect the verify response the caller is waiting on.

`ses.ts`'s `resolveAccountEvent(newEmail, previousEmail)` decides which:

- `previousEmail` absent/null → **sign-up** ("A new Meridian install just
  verified its email: `<email>`").
- `previousEmail` present and different → **email changed** ("...changed its
  verified email from `<old>` to `<new>`").
- `previousEmail` present and identical to the new email → **no-op**, nothing
  sent (a "Change email" re-entering the same address that's already on
  file).

`previousEmail` is sent by the client (`tray/src-tauri/src/commands/otp.rs`'s
`confirm_account_otp`, reading `commands::account::read_account_email()`
before the request) and is purely informational — this Worker has no durable
account state of its own to derive it from independently, and doesn't need
one: unlike a routine sign-in, every verify in this app is either a genuine
one-time capture or a deliberate "Change email" action (there is no
session/re-login concept), so there's no repeat-noise case to dedup against
and no once-per-day flag like `ALERT_EMAIL`'s.

## Turnstile (conditional, per the plan)

**Turnstile support is wired but CONDITIONAL: whether the client actually
sends a token depends on a separate frontend/Tauri feasibility spike this
Worker does not block on. The rate-limiting layer above is the mechanism
that holds either way** — a request with no `turnstileToken` at all is
accepted purely on bearer-auth + rate limits, exactly as if Turnstile didn't
exist.

Two independent gates in `turnstile.ts`, both must pass for verification to
actually run:

1. The request body includes a `turnstileToken` at all. If absent, `index.ts`
   never calls `turnstile.ts` — the send proceeds on bearer-auth +
   rate-limits alone (the plan's explicit "if absent, proceed without it").
2. `TURNSTILE_SECRET_KEY` (a Worker secret, **not** one of the plan's
   originally-listed secrets — added here since Turnstile wasn't in scope
   when that table was written) is actually configured. If unset — the
   expected state until/unless the frontend spike succeeds and a Turnstile
   site is provisioned — verification is a no-op that returns `true` and
   logs a warning, rather than rejecting every send because a feature
   nothing has enabled yet looks "misconfigured."

**Resolved, not specified by the plan: a token that IS present but FAILS
verification is rejected with 403**, rather than silently ignored — fail
closed on invalid input, consistent with this repo's conventions elsewhere.
A present-and-valid token verifies as usual; an absent token skips
verification entirely (gate 1); an unconfigured secret with a present token
also proceeds (gate 2), since there's nothing to validate against yet.

## Anti-abuse summary

Defense in depth, in the order a request actually passes through them:

1. Bearer token (attestation, not a strong secret — see above)
2. Turnstile, if/when wired up client-side (optional, see above)
3. Three independent KV rate limits (per-email/per-IP/global), checked
   before any code is generated or SES is called
4. HMAC-peppered code hash + 5-attempt cap on verify

## Secrets / config split

Mirrors the existing `ops/central-observability` convention (public
`vars.*` vs. `wrangler secret put`-only values) and the plan's own table:

| Name | Where | Notes |
|---|---|---|
| `OTP_CLIENT_TOKEN` | `wrangler secret put` (both envs) | Bearer token the tray sends |
| `OTP_CODE_PEPPER` | `wrangler secret put` (both envs) | HMAC pepper for code hashing |
| `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` / `AWS_REGION` | `wrangler secret put` (both envs) | Scoped to an IAM policy permitting only `ses:SendEmail` on the verified identity |
| `TURNSTILE_SECRET_KEY` | `wrangler secret put` (both envs), optional | Unset until/unless the frontend spike lands — see "Turnstile" above |
| `CI_TEST_TOKEN` | `wrangler secret put --env staging` **only** | Staging-only auth token that also unlocks the `/otp/send` code echo — see below |
| `ENVIRONMENT`, `FROM_ADDRESS`, `FROM_NAME`, `OTP_TTL_S`, `MAX_VERIFY_ATTEMPTS`, `RL_EMAIL_PER_DAY`, `RL_IP_PER_HOUR`, `RL_GLOBAL_PER_DAY`, `ALERT_THRESHOLD_PCT`, `ALERT_EMAIL`, `NOTIFY_EMAIL` | `wrangler.jsonc` `vars` | Public, tunable without touching code |

### Staging-only code echo

`scripts/deploy-otp-worker.sh`'s happy-path send→verify smoke test needs to
read the generated code without a real inbox. `/otp/send` echoes the code in
its response body (`{ ok: true, code }`) under **two** conditions, both
checked at the point of response, not just at auth time:

```ts
if (env.ENVIRONMENT === "staging" && auth.isCiTestToken) {
  return ok({ code });
}
```

`env.ENVIRONMENT !== "staging"` is an explicit runtime guard, not just "the
secret happens to be unset" — a `CI_TEST_TOKEN` secret mistakenly present on
production would still never trigger the echo there. And the echo never
fires for the normal `OTP_CLIENT_TOKEN` bearer, even on staging — only the
distinct `CI_TEST_TOKEN` unlocks it (see `auth.ts`'s `isCiTestToken` flag).
The production bearer token can never reach this path, on any environment.

## Testing

Unit tests (`src/__tests__/*.test.ts`) run inside a real Miniflare-simulated
Workers runtime via **`@cloudflare/vitest-plugin`** — no Cloudflare account
or network access required, everything runs locally against
`wrangler.jsonc`'s binding shapes.

**Resolved, not specified by the plan: `@cloudflare/vitest-plugin`, not
`@cloudflare/vitest-pool-workers`.** There is no existing Workers test
convention anywhere else in this repo to mirror (`ui/` and
`packages/meridian-mcp/` both use plain Node-based test runners with no
bundler-aware pool), so this is a new precedent for the repo, not an
established one. `@cloudflare/vitest-pool-workers` (the package named in the
plan's discussion and in most existing docs/tutorials as of this writing) no
longer exports `defineWorkersConfig` from `/config` as of its `0.22.x`
line — that API was replaced by a plugin-based config
(`cloudflareTest()` from `@cloudflare/vitest-plugin`, used in `vitest.config.mts`
via `defineConfig({ plugins: [cloudflareTest(...)] })`) to match Vitest 4's
plugin architecture. This was chosen over falling back to hand-rolled fakes
for everything because it wasn't more than trivial friction to wire up once
the correct current package was identified, and it gives real coverage of
`crypto.subtle.timingSafeEqual` and the actual `KVNamespace` binding
(`kv.test.ts`) rather than a hand-mocked substitute for either.

Coverage priorities, highest first:

- `otp.test.ts` — the TTL-preservation invariant on a wrong guess, the
  exact-5th-attempt exhaustion boundary, and the "already exhausted, right
  code doesn't matter" case. This is the module a regression here would be
  most dangerous in.
- `auth.test.ts` — the empty-secret-never-passes and
  CI-token-never-works-off-staging cases.
- `ratelimit.test.ts` — window-open/preserve/reset boundaries and the
  three-scope precedence order.
- `turnstile.test.ts` — both conditional gates, and fail-closed on network
  error / non-2xx / explicit failure.
- `ses.test.ts` — never logging a raw SES error body (which can echo the
  destination email in sandbox mode), and that the code never leaks into
  the request URL.
- `kv.test.ts` — a real KV round-trip (not a fake), plus the 60s-floor
  clamp math.

```bash
npm install
npm run typecheck   # tsc --noEmit
npm test            # vitest run, inside simulated Workers runtime
npx wrangler deploy --dry-run              # validates wrangler.jsonc, no auth needed
npx wrangler deploy --dry-run --env staging
```

All four commands above were run as part of building this Worker and pass
with no Cloudflare account access — `--dry-run` bundles and validates
`wrangler.jsonc` fully offline.

## Manual steps before first deploy

None of the following can be done from this code — they need an operator
with AWS/Cloudflare account access:

1. **KV namespaces.** `npx wrangler kv namespace create OTP_KV` (production)
   and `npx wrangler kv namespace create OTP_KV --env staging`, then paste
   the two returned ids into `wrangler.jsonc`'s
   `REPLACE_ME_KV_NAMESPACE_ID_PRODUCTION` / `_STAGING` placeholders.
2. **SES sending identity.** Verify `auth.meridiona.com` (or whatever
   subdomain is chosen) in the SES console — adds DKIM/SPF DNS records to
   the existing Cloudflare-managed zone, same pattern as `telemetry.`/
   `observe.`. Update `wrangler.jsonc`'s `FROM_ADDRESS` if the real verified
   address differs from the `otp@auth.meridiona.com` placeholder.
3. **SES production access.** New accounts start in a sandbox: 200
   emails/24h, 1/sec, and can only send to pre-verified recipients —
   unusable for real users. File an AWS Support case to request production
   access before PR 2 (the client cut) ships to real users. Confirm the
   granted quota in the SES console once approved.
4. **IAM credentials.** Create a narrowly-scoped IAM user/policy granting
   only `ses:SendEmail` on the verified identity.
5. **Secrets**, run for both environments (default + `--env staging`) unless
   noted:
   ```bash
   npx wrangler secret put OTP_CLIENT_TOKEN
   npx wrangler secret put OTP_CODE_PEPPER
   npx wrangler secret put AWS_ACCESS_KEY_ID
   npx wrangler secret put AWS_SECRET_ACCESS_KEY
   npx wrangler secret put AWS_REGION
   npx wrangler secret put CI_TEST_TOKEN --env staging   # staging only
   # optional, only once the Turnstile frontend spike lands:
   npx wrangler secret put TURNSTILE_SECRET_KEY
   ```
6. **Deploy + verify:**
   ```bash
   npm run deploy:staging   # wrangler deploy --env staging
   bash ../../scripts/deploy-otp-worker.sh --verify-only <staging-url>
   npm run deploy           # wrangler deploy (production)
   bash ../../scripts/deploy-otp-worker.sh --verify-only <production-url>
   ```
