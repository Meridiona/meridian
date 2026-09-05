//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
/**
 * Meridian's OTP send/verify Worker — the backend for the setup wizard's
 * one-time email capture step (see the parent plan,
 * `giggly-jumping-hopcroft.md`, Part 1).
 *
 * Exactly two routes exist; everything else 404s:
 *   `POST /otp/send`   `{ email, turnstileToken? }`
 *   `POST /otp/verify` `{ email, code }`
 * Both require `Authorization: Bearer <token>` — see `auth.ts`.
 *
 * This file is intentionally thin: routing, request-body validation, and
 * gate ordering only. Every gate below (`auth.ts`, `turnstile.ts`,
 * `ratelimit.ts`, `otp.ts`) is independently unit-tested as a pure function;
 * this is the only place they're wired to a real `KVNamespace` and real
 * `fetch` calls (`kv.ts`, `ses.ts`, `turnstile.ts`).
 *
 * # Who calls this
 * - `tray/src-tauri/src/commands/otp.rs` (Part 2 of the plan, out of this
 *   Worker's scope) — `request_account_otp` / `confirm_account_otp`.
 *
 * # Related
 * - README.md — design rationale, KV schema, status-code mapping, manual
 *   deploy prerequisites.
 * - `scripts/deploy-otp-worker.sh` — post-deploy smoke test against these
 *   exact routes/status codes.
 */

import { checkBearerAuth } from "./auth";
import { emailHash, normalizeEmail } from "./email";
import {
  getCounter,
  getOtpRecord,
  deleteOtpRecord,
  hasAlertBeenSent,
  markAlertSent,
  putCounter,
  putOtpRecord,
} from "./kv";
import { createOtpRecord, generateCode, hashCode, verifyOtpAttempt } from "./otp";
import {
  evaluateRateLimits,
  incrementCounter,
  shouldSendRateLimitAlert,
  utcDateString,
  type RateLimitScope,
} from "./ratelimit";
import {
  badRequest,
  forbidden,
  gone,
  notFound,
  ok,
  rateLimited,
  serviceUnavailable,
  unauthorized,
} from "./responses";
import { resolveAccountEvent, sendAccountEventEmail, sendOtpEmail, sendRateLimitAlertEmail } from "./ses";
import { verifyTurnstileToken } from "./turnstile";

const HOUR_MS = 60 * 60 * 1000;
const DAY_MS = 24 * HOUR_MS;
/** Fixed KV cleanup TTL for `global:sends:<date>` — see `kv.ts`'s `putCounter`. */
const GLOBAL_COUNTER_KV_TTL_S = 90_000;

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
 * Fire the once-per-UTC-day "approaching the global cap" alert if
 * `newGlobalCount` just crossed `ALERT_THRESHOLD_PCT` of `RL_GLOBAL_PER_DAY`.
 * Checked AFTER the global counter is durably written, using the
 * already-incremented count — never blocks or affects the OTP send's own
 * response (the caller wires this into `ctx.waitUntil`, fire-and-forget).
 * The `alert:sent:<date>` KV flag is what makes this once-per-day rather
 * than once-per-request-past-threshold.
 */
async function maybeSendRateLimitAlert(env: Env, newGlobalCount: number, dateKey: string): Promise<void> {
  if (!env.ALERT_EMAIL) return;
  const cap = Number(env.RL_GLOBAL_PER_DAY);
  const thresholdPct = Number(env.ALERT_THRESHOLD_PCT);
  const date = dateKey.replace("global:sends:", "");

  const alreadySentToday = await hasAlertBeenSent(env.OTP_KV, date);
  if (!shouldSendRateLimitAlert(newGlobalCount, cap, thresholdPct, alreadySentToday)) return;

  const sent = await sendRateLimitAlertEmail(newGlobalCount, cap, thresholdPct, env);
  if (sent) {
    await markAlertSent(env.OTP_KV, date, GLOBAL_COUNTER_KV_TTL_S);
  } else {
    console.error("otp-worker: rate-limit alert email failed to send", { newGlobalCount, cap });
  }
}

async function handleSend(request: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
  const auth = checkBearerAuth(request.headers.get("Authorization"), env);
  if (!auth.ok) return unauthorized();

  const body = await readJsonBody(request);
  if (!body) return badRequest("invalid_json");

  const email = normalizeEmail(body.email);
  if (!email) return badRequest("invalid_email");

  const turnstileToken = body.turnstileToken;
  if (turnstileToken !== undefined && typeof turnstileToken !== "string") {
    return badRequest("invalid_turnstile_token");
  }

  const ip = clientIp(request);

  // Gate: Turnstile, only when the client actually sent a token (see
  // turnstile.ts's module header for the full conditional-support story).
  if (typeof turnstileToken === "string" && turnstileToken.length > 0) {
    const passed = await verifyTurnstileToken(
      turnstileToken,
      env.TURNSTILE_SECRET_KEY,
      ip !== "unknown" ? ip : undefined,
    );
    if (!passed) return forbidden("turnstile_failed");
  }

  const now = Date.now();
  const hash = await emailHash(email);
  const dateKey = `global:sends:${utcDateString(now)}`;

  const [emailCounter, ipCounter, globalCounter] = await Promise.all([
    getCounter(env.OTP_KV, `rl:email:${hash}`),
    getCounter(env.OTP_KV, `rl:ip:${ip}`),
    getCounter(env.OTP_KV, dateKey),
  ]);

  const decision = evaluateRateLimits({
    emailRecord: emailCounter,
    ipRecord: ipCounter,
    globalRecord: globalCounter,
    now,
    caps: {
      email: Number(env.RL_EMAIL_PER_DAY),
      ip: Number(env.RL_IP_PER_HOUR),
      global: Number(env.RL_GLOBAL_PER_DAY),
    },
  });
  if (!decision.allowed) {
    const scope: RateLimitScope = decision.scope ?? "global";
    console.warn("otp-worker: send rate limited", { scope, emailHashPrefix: hash.slice(0, 8) });
    return rateLimited(scope);
  }

  // Counters are persisted BEFORE the SES call, deliberately: the caps exist
  // for cost/abuse containment against ATTEMPTED sends, so a run of SES
  // failures (an outage, a bad credential) must still count against budget —
  // otherwise an attacker (or a genuine outage) could drive unlimited
  // send-attempt traffic for free by ensuring every attempt "fails" cheaply.
  const ttlMs = Number(env.OTP_TTL_S) * 1000;
  const code = generateCode();
  const codeHash = await hashCode(code, env.OTP_CODE_PEPPER);
  const record = createOtpRecord(codeHash, now, ttlMs);

  const newGlobalCounter = incrementCounter(globalCounter, now, DAY_MS);

  await Promise.all([
    putOtpRecord(env.OTP_KV, hash, record, now),
    putCounter(env.OTP_KV, `rl:email:${hash}`, incrementCounter(emailCounter, now, DAY_MS), now),
    putCounter(env.OTP_KV, `rl:ip:${ip}`, incrementCounter(ipCounter, now, HOUR_MS), now),
    putCounter(env.OTP_KV, dateKey, newGlobalCounter, now, GLOBAL_COUNTER_KV_TTL_S),
  ]);

  // Fire-and-forget: never let the alert path slow down or fail the actual
  // OTP send. `waitUntil` keeps the Worker alive long enough to finish it
  // after the response has already been returned to the caller.
  ctx.waitUntil(maybeSendRateLimitAlert(env, newGlobalCounter.count, dateKey));

  const ttlMinutes = Math.max(1, Math.round(Number(env.OTP_TTL_S) / 60));
  const sent = await sendOtpEmail(email, code, ttlMinutes, env);
  if (!sent) {
    return serviceUnavailable("email_delivery_failed");
  }

  // Staging-only code echo for scripts/deploy-otp-worker.sh's happy-path
  // send->verify probe. Both conditions are required and checked here, not
  // just at auth time, so this can never fire on production even if a
  // CI_TEST_TOKEN secret were mistakenly present there.
  if (env.ENVIRONMENT === "staging" && auth.isCiTestToken) {
    return ok({ code });
  }
  return ok();
}

async function handleVerify(request: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
  const auth = checkBearerAuth(request.headers.get("Authorization"), env);
  if (!auth.ok) return unauthorized();

  const body = await readJsonBody(request);
  if (!body) return badRequest("invalid_json");

  const email = normalizeEmail(body.email);
  if (!email) return badRequest("invalid_email");

  const rawCode = body.code;
  if (typeof rawCode !== "string" || !/^\d{6}$/.test(rawCode)) {
    return badRequest("invalid_code");
  }

  // Optional, client-supplied, purely informational — see resolveAccountEvent's
  // doc. An absent or unparseable value just reads as "no prior email".
  const previousEmail = normalizeEmail(body.previousEmail);

  const now = Date.now();
  const hash = await emailHash(email);
  const [record, providedHash] = await Promise.all([
    getOtpRecord(env.OTP_KV, hash),
    hashCode(rawCode, env.OTP_CODE_PEPPER),
  ]);

  const maxAttempts = Number(env.MAX_VERIFY_ATTEMPTS);
  const outcome = verifyOtpAttempt(record, providedHash, now, maxAttempts);

  switch (outcome.kind) {
    case "verified": {
      await deleteOtpRecord(env.OTP_KV, hash);
      const event = resolveAccountEvent(email, previousEmail);
      if (event && env.NOTIFY_EMAIL) {
        ctx.waitUntil(
          sendAccountEventEmail(event, env).then((sent) => {
            if (!sent) console.error("otp-worker: account-event notification failed to send", { kind: event.kind });
          }),
        );
      }
      return ok({ verified: true });
    }
    case "wrong":
      await putOtpRecord(env.OTP_KV, hash, outcome.nextRecord, now);
      return ok({ verified: false, attemptsRemaining: outcome.attemptsRemaining });
    case "exhausted":
      await deleteOtpRecord(env.OTP_KV, hash);
      return gone("code_expired_or_not_found");
    case "not_found_or_expired":
      return gone("code_expired_or_not_found");
  }
}

export default {
  async fetch(request: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
    const url = new URL(request.url);
    try {
      if (request.method === "POST" && url.pathname === "/otp/send") {
        return await handleSend(request, env, ctx);
      }
      if (request.method === "POST" && url.pathname === "/otp/verify") {
        return await handleVerify(request, env, ctx);
      }
      return notFound();
    } catch (err) {
      console.error("otp-worker: unhandled error", { error: String(err), path: url.pathname });
      return serviceUnavailable("internal_error");
    }
  },
} satisfies ExportedHandler<Env>;
