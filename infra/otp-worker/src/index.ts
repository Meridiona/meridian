//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
/**
 * Meridian's account-email-capture Worker — the backend for the setup
 * wizard's one-time email capture step (see the parent plan,
 * `giggly-jumping-hopcroft.md`, Part 1).
 *
 * Exactly one route exists; everything else 404s:
 *   `POST /otp/capture` `{ email, previousEmail? }`
 * Requires `Authorization: Bearer <token>` — see `auth.ts`.
 *
 * This used to be two Workers-worth of routes: `/otp/send` (generate a code,
 * email it via SES) and `/otp/verify` (check it). AWS SES was never approved
 * for production sending, so the send/verify round-trip could fail outright
 * for real users' arbitrary addresses — blocking sign-up rather than just
 * adding delay. `/otp/capture` replaced both: no code generation, no SES
 * call, just rate-limiting and the same internal "someone signed up"
 * notification `/otp/verify` used to fire on a verified code (see
 * `handleCapture` and `resend.ts`). Once `/otp/capture` became the only
 * caller either route ever had, `/otp/send`/`/otp/verify` and everything that
 * only existed for them (OTP code generation/hashing, the SES client,
 * Turnstile, the rate-limit-approaching alert email) were removed rather than
 * kept as unreachable code — see `otp.ts`'s prior history if code
 * verification is ever reinstated.
 *
 * This file is intentionally thin: routing, request-body validation, and
 * gate ordering only. `ratelimit.ts`'s decision logic is independently
 * unit-tested as pure functions; this is the only place they're wired to a
 * real Durable Object (`rate-limiter.ts`'s `RateLimitCounter`, one instance
 * per email hash / IP / UTC date, which is what makes the three caps
 * race-free under concurrency — see that module's header).
 *
 * # Who calls this
 * - `tray/src-tauri/src/commands/otp.rs` (Part 2 of the plan, out of this
 *   Worker's scope) — `capture_account_email`.
 *
 * # Related
 * - README.md — design rationale, KV schema, status-code mapping, manual
 *   deploy prerequisites.
 * - `scripts/deploy-otp-worker.sh` — post-deploy smoke test against these
 *   exact routes/status codes.
 */

import { checkBearerAuth } from "./auth";
import { emailHash, normalizeEmail } from "./email";
import { utcDateString, type RateLimitScope } from "./ratelimit";
import type { RateLimitCounter } from "./rate-limiter";
import { badRequest, notFound, ok, rateLimited, serviceUnavailable, unauthorized } from "./responses";
import { resolveAccountEvent, sendAccountEventEmail } from "./resend";

const HOUR_MS = 60 * 60 * 1000;
const DAY_MS = 24 * HOUR_MS;

/**
 * Route `key` to its Durable Object instance and call `tryConsume` on it.
 * `idFromName` deterministically maps the same string to the same instance
 * every time, which is what makes this a per-key counter rather than a
 * global one — see `rate-limiter.ts`'s module header for why routing
 * through a DO closes the TOCTOU race a direct KV read/write had.
 */
function tryConsume(
  binding: DurableObjectNamespace<RateLimitCounter>,
  key: string,
  now: number,
  windowMs: number,
  cap: number,
): Promise<boolean> {
  const stub = binding.get(binding.idFromName(key));
  return stub.tryConsume(now, windowMs, cap);
}

function clientIp(request: Request): string {
  return request.headers.get("CF-Connecting-IP") ?? "unknown";
}

/** Parse and loosely-type the JSON body; `null` on anything unparseable or non-object. */
async function readJsonBody(request: Request): Promise<Record<string, unknown> | null> {
  let parsed: unknown;
  try {
    parsed = await request.json();
  } catch {
    return null;
  }
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) return null;
  return parsed as Record<string, unknown>;
}

/**
 * `/otp/capture`: no code, no SES call — just rate-limit (own counters, so
 * this traffic is independent of any budget the old `/otp/send` route used
 * to spend) and fire the internal "signed up / changed email" notification.
 * Always returns `ok()` once past rate-limiting: there is nothing further to
 * verify, and a failed notification (`sendAccountEventEmail` never throws,
 * see `resend.ts`) must never turn into a failed capture from the caller's
 * point of view.
 *
 * The three caps are checked (and atomically consumed) IN ORDER — email,
 * then IP, then global — stopping at the first one that's already at cap.
 * Each `tryConsume` call is its own atomic check-and-increment (see
 * `rate-limiter.ts`), so a request rejected on, say, the IP cap has already
 * consumed one unit of the email cap's budget. That's a deliberate, minor
 * trade-off: the alternative (peek all three, decide, then increment only
 * the allowed ones) reopens a race between the peek and the increment,
 * which is exactly the bug this rewrite exists to close. Over-counting on
 * the rejection path only ever makes the limiter MORE conservative, never
 * less — it cannot let more traffic through than the caps allow.
 */
async function handleCapture(request: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
  const auth = checkBearerAuth(request.headers.get("Authorization"), env);
  if (!auth.ok) return unauthorized();

  const body = await readJsonBody(request);
  if (!body) return badRequest("invalid_json");

  const email = normalizeEmail(body.email);
  if (!email) return badRequest("invalid_email");

  // Optional, client-supplied, purely informational — see resolveAccountEvent's
  // doc. An absent or unparseable value just reads as "no prior email".
  const previousEmail = normalizeEmail(body.previousEmail);

  const ip = clientIp(request);
  const now = Date.now();
  const hash = await emailHash(email);

  const caps: Array<{ scope: RateLimitScope; key: string; windowMs: number; cap: number }> = [
    { scope: "email", key: `rl:capture:email:${hash}`, windowMs: DAY_MS, cap: Number(env.RL_EMAIL_PER_DAY) },
    { scope: "ip", key: `rl:capture:ip:${ip}`, windowMs: HOUR_MS, cap: Number(env.RL_IP_PER_HOUR) },
    {
      scope: "global",
      key: `global:captures:${utcDateString(now)}`,
      windowMs: DAY_MS,
      cap: Number(env.RL_GLOBAL_PER_DAY),
    },
  ];
  for (const { scope, key, windowMs, cap } of caps) {
    const allowed = await tryConsume(env.RATE_LIMITER, key, now, windowMs, cap);
    if (!allowed) {
      console.warn("otp-worker: capture rate limited", { scope, emailHashPrefix: hash.slice(0, 8) });
      return rateLimited(scope);
    }
  }

  const event = resolveAccountEvent(email, previousEmail);
  if (event && env.NOTIFY_EMAIL) {
    ctx.waitUntil(
      sendAccountEventEmail(event, env).then((sent) => {
        if (!sent) console.error("otp-worker: account-event notification failed to send", { kind: event.kind });
      }),
    );
  }
  return ok();
}

/** Re-exported so `wrangler.jsonc`'s `durable_objects` binding can find the
 *  class — Cloudflare resolves a DO binding's `class_name` against an export
 *  of the Worker's main module, not `rate-limiter.ts` directly. */
export { RateLimitCounter } from "./rate-limiter";

export default {
  async fetch(request: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
    const url = new URL(request.url);
    try {
      if (request.method === "POST" && url.pathname === "/otp/capture") {
        return await handleCapture(request, env, ctx);
      }
      return notFound();
    } catch (err) {
      console.error("otp-worker: unhandled error", { error: String(err), path: url.pathname });
      return serviceUnavailable("internal_error");
    }
  },
} satisfies ExportedHandler<Env>;
