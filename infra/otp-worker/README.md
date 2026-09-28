# otp-worker

Cloudflare Worker backing Meridian's email capture step (the setup wizard's
replacement for Clerk — see the parent plan, `giggly-jumping-hopcroft.md`, for
the full "why"). No accounts, no sessions, no sign-out — capture the typed
email once, store it locally, never re-check.

**There is no code-verification step.** This Worker used to also have
`/otp/send` (generate a 6-digit code, email it via SES) and `/otp/verify`
(check it). AWS SES was never approved for production sending (still
sandboxed to individually-verified recipient addresses), which meant
`/otp/send`'s code delivery could fail outright for real users — blocking
sign-up rather than adding delay. The desktop app called `/otp/capture`
instead from the start: no code generated, no SES call, just the same
internal "someone signed up" notification `/otp/verify` used to fire on a
verified code. Once `/otp/capture` became the only caller either route ever
had, `/otp/send`/`/otp/verify` and everything that only existed to support
them (OTP code generation/hashing, the SES client, Turnstile, the
rate-limit-approaching alert email) were removed rather than kept as
unreachable code. Check this file's git history if code verification is ever
reinstated — the removed design is fully described there.

This is the **first live Cloudflare Worker in this repo.** Read "Why this
design" below before changing anything auth- or rate-limit-related — a prior
Worker (`infra/hf-proxy`, since deleted) shipped unauthenticated with no rate
limit, got hammered for 173,088 requests in a day against a 100k/day
account-wide cap, and took `meridiona.com` down. CLAUDE.md's Hard Rules
section has the full incident writeup; this Worker exists specifically not to
repeat it.

## Routes

Exactly one exists. Everything else — wrong path, wrong method — gets a plain
404. Requires `Authorization: Bearer <token>`.

| Route | Body | Purpose |
|---|---|---|
| `POST /otp/capture` | `{ email, previousEmail? }` | Rate-limit + fire the account-event notification, no code |

`previousEmail` is optional and purely informational — the client's best
knowledge of the address it had on file before this call, used only to
decide what (if anything) to tell `NOTIFY_EMAIL` about (see "Account-event
notification" below). It is never used for any security decision.

### Status codes

| Status | Body | Meaning |
|---|---|---|
| 200 | `{ ok: true }` | Rate limits passed; notification fired (or skipped if unconfigured) |
| 400 | `{ error: "invalid_email" \| "invalid_json" }` | Malformed request |
| 401 | `{ error: "unauthorized" }` | Missing/wrong bearer token |
| 429 | `{ error: "rate_limited", scope: "email" \| "ip" \| "global" }` | One of the three capture caps tripped |

## Why this design (hf-proxy postmortem, applied)

Four things this Worker does that `infra/hf-proxy` didn't, each mapped to a
line item in CLAUDE.md's Hard Rules:

1. **Authenticates every request.** `auth.ts` checks `Authorization: Bearer`
   before any body parsing or rate-limit check, with no unauthenticated path.
   Empty/unconfigured secrets never match (guards the "both sides blank"
   bypass).
2. **Allowlists the paths it serves.** The router in `index.ts` is an
   exhaustive `if/else 404` — there is no default-allow branch.
3. **Rate-limits.** Three independent Durable-Object-backed caps (per-email,
   per-IP, global-daily) gate the one route before anything else happens.
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

## Rate-limit counters (Durable Objects)

Three independent caps, checked and atomically consumed in this order:
per-email, per-IP, then global. Each is a `RateLimitCounter` Durable Object
instance (`src/rate-limiter.ts`), addressed by `idFromName(key)` — the same
key strings a KV-based implementation would have used as keys:

| Key | Cap | Window |
|---|---|---|
| `rl:capture:email:<sha256(normalizeEmail(email))>` | `RL_EMAIL_PER_DAY` | rolling 24h from first capture |
| `rl:capture:ip:<CF-Connecting-IP>` | `RL_IP_PER_HOUR` | rolling 1h from first capture |
| `global:captures:<UTC date>` | `RL_GLOBAL_PER_DAY` | one instance per calendar day |

**This used to be three plain KV counters, read-then-written as two separate
calls from the Worker.** A security review found that racy — two concurrent
requests could both read the same under-cap count and both "win," letting the
cap be exceeded. Cloudflare Workers KV has no atomic increment or
compare-and-swap, so no amount of KV-only cleverness closes that gap: a
Durable Object is the standard fix, because Cloudflare guarantees a single DO
instance's storage operations are serialized against concurrent calls to
that same instance (the "input/output gate" — see `rate-limiter.ts`'s module
header for the full mechanics, and its test file's concurrency test, which
fires 20 simultaneous requests at one counter and asserts exactly `cap` of
them are ever allowed through).

Each counter still embeds its own authoritative `expiresAt` (epoch ms) — the
same rolling-window design the old KV implementation used — and an `alarm()`
handler deletes the counter once its window has passed, replacing the
passive cleanup KV's `expirationTtl` used to provide.

**Migrating an existing deployment:** the old `OTP_KV` namespace and its
`kv_namespaces` binding are gone from `wrangler.jsonc`; the KV namespaces
themselves (`wrangler kv namespace create`'d per "Manual steps" below) are
now orphaned Cloudflare resources and can be deleted by hand once you've
confirmed the new Durable-Object-backed deploy is healthy
(`wrangler kv namespace delete`, an operator action outside this Worker's
code).

## Account-event notification

On every successful `/otp/capture`, `handleCapture` fires a notification to
`NOTIFY_EMAIL` telling the team that an install signed up or changed its
email. This rides on `ctx.waitUntil` — fire-and-forget, never awaited inline,
so a failed notification can never affect the capture response the caller is
waiting on.

**This email goes via Resend** (`resend.ts`). The marketing site has sent the
identical notification since June — `Meridian Sign-ins
<notify@meridiona.com>` → `adithya@meridiona.com`, subject `New sign-up:
<email>` — and routing the desktop app's copy through the same provider keeps
web and desktop sign-ups in one inbox with one sender identity. An internal
notification to one address is a couple of dozen a day at most, well within
Resend's free tier.

The body deliberately mirrors the website's format (plain text, no HTML part;
address on line 1, source, then one status line). Where the web version
carries `Clerk user id: …`, the desktop app has no equivalent since Clerk was
removed, so line 2 names the source — which is also what distinguishes a
desktop notification from a web one at a glance.

`resend.ts`'s `resolveAccountEvent(newEmail, previousEmail)` decides which:

- `previousEmail` absent/null → **sign-up**, subject `New sign-up: <email>`
  (byte-identical to the website's convention).
- `previousEmail` present and different → **email changed**, subject
  `Email changed: <old> -> <new>`. No web equivalent exists for this case.
- `previousEmail` present and identical to the new email → **no-op**, nothing
  sent (a "Change email" re-entering the address already on file must not
  claim something changed).

`previousEmail` is sent by the client (`tray/src-tauri/src/commands/otp.rs`'s
`capture_account_email`, reading `commands::account::read_account_email()`
before the request) and is purely informational — this Worker has no durable
account state of its own to derive it from independently, and doesn't need
one: unlike a routine sign-in, every capture in this app is either a genuine
one-time capture or a deliberate "Change email" action (there is no
session/re-login concept), so there's no repeat-noise case to dedup against.

## Anti-abuse summary

Defense in depth, in the order a request actually passes through them:

1. Bearer token (attestation, not a strong secret — see above)
2. Three independent Durable-Object-backed rate limits (per-email/per-IP/global)

## Secrets / config split

Mirrors the existing `ops/central-observability` convention (public
`vars.*` vs. `wrangler secret put`-only values):

| Name | Where | Notes |
|---|---|---|
| `OTP_CLIENT_TOKEN` | `wrangler secret put` (both envs) | Bearer token the tray sends |
| `RESEND_API_KEY` | `wrangler secret put` (both envs) | Sending-access key scoped to the `meridiona.com` domain, for the account-event notification |
| `RL_EMAIL_PER_DAY`, `RL_IP_PER_HOUR`, `RL_GLOBAL_PER_DAY`, `NOTIFY_EMAIL`, `NOTIFY_FROM` | `wrangler.jsonc` `vars` | Public, tunable without touching code |

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
established one. `@cloudflare/vitest-pool-workers` (the package named in most
existing docs/tutorials as of this writing) no longer exports
`defineWorkersConfig` from `/config` as of its `0.22.x` line — that API was
replaced by a plugin-based config (`cloudflareTest()` from
`@cloudflare/vitest-plugin`, used in `vitest.config.mts` via
`defineConfig({ plugins: [cloudflareTest(...)] })`) to match Vitest 4's plugin
architecture.

Coverage priorities, highest first:

- `rate-limiter.test.ts` — a real Durable Object round-trip (not a fake),
  window-open/preserve/reset boundaries, and the concurrency test that
  proves the TOCTOU fix: 20 simultaneous requests against one counter never
  let more than `cap` through.
- `auth.test.ts` — the empty-secret-never-passes case.
- `ratelimit.test.ts` — the pure window-open/preserve/reset math
  `rate-limiter.ts` builds on.
- `index.test.ts` — the router/auth wiring against the real `fetch` handler.

```bash
npm install
npm run typecheck   # tsc --noEmit
npm test            # vitest run, inside simulated Workers runtime
npx wrangler deploy --dry-run              # validates wrangler.jsonc, no auth needed
npx wrangler deploy --dry-run --env staging
```

## Manual steps before first deploy

None of the following can be done from this code — they need an operator
with Cloudflare/Resend account access. (The `RateLimitCounter` Durable Object
needs no manual provisioning step — `wrangler deploy` creates it from the
`durable_objects`/`migrations` blocks in `wrangler.jsonc` on first deploy.)

1. **Secrets**, run for both environments (default + `--env staging`):
   ```bash
   npx wrangler secret put OTP_CLIENT_TOKEN
   npx wrangler secret put RESEND_API_KEY
   ```
2. **Deploy + verify:**
   ```bash
   npm run deploy:staging   # wrangler deploy --env staging
   bash ../../scripts/deploy-otp-worker.sh --verify-only <staging-url>
   npm run deploy           # wrangler deploy (production)
   bash ../../scripts/deploy-otp-worker.sh --verify-only <production-url>
   ```
