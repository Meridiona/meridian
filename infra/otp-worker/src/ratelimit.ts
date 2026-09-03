//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
/**
 * Pure rate-limit decision logic for `/otp/send`, kept separate from the KV
 * read/write glue in `index.ts` so the caps themselves are unit-testable
 * without mocking a `KVNamespace`.
 *
 * Three independent caps, checked in this order (cheapest/most-specific
 * first, matching the plan's KV schema): per-email, per-IP, then global.
 * `rl:email:<hash>` and `rl:ip:<ip>` are rolling fixed windows that open on
 * the first send and reset only once that window's `expiresAt` has passed —
 * NOT a calendar-boundary reset — which is why, like `otp.ts`'s record, the
 * expiry is carried in the value itself rather than relied on purely via
 * KV's own TTL (KV requires re-specifying `expirationTtl` on every write, so
 * a naive re-put on each increment would either reset the window every
 * request or silently drop expiry entirely).
 *
 * `global:sends:<date>` is different: its KV key is itself namespaced by UTC
 * date (see {@link utcDateString}), so the "window" is just "this key exists
 * for one calendar day" — no embedded expiry needed, a plain KV
 * `expirationTtl` for cleanup is enough (see the plan's 90000s, deliberately
 * longer than a day as a timezone-drift safety margin, not a precise window).
 *
 * # Who calls this
 * - `index.ts`'s `/otp/send` handler, after auth and body validation, before
 *   generating a code or spending an SES send.
 *
 * # Related
 * - `otp.ts` — the sibling pure state machine for the OTP record itself.
 */

/** A rolling fixed-window counter (`rl:email:*`, `rl:ip:*`). */
export interface CounterRecord {
  count: number;
  /** Epoch ms — when this window resets, opening a fresh one on next write. */
  expiresAt: number;
}

/** UTC calendar-day string (`YYYY-MM-DD`) — the `global:sends:<date>` key suffix. */
export function utcDateString(now: number): string {
  return new Date(now).toISOString().slice(0, 10);
}

/**
 * Fold one more send into a rolling counter. Starts a fresh window (count 1)
 * if there is no existing record or the existing window has already expired;
 * otherwise increments in place, leaving `expiresAt` untouched.
 */
export function incrementCounter(existing: CounterRecord | null, now: number, windowMs: number): CounterRecord {
  if (!existing || now >= existing.expiresAt) {
    return { count: 1, expiresAt: now + windowMs };
  }
  return { count: existing.count + 1, expiresAt: existing.expiresAt };
}

/** Whether a counter is currently at or past its cap. An expired/absent window is never over cap. */
export function isOverCap(record: CounterRecord | null, now: number, cap: number): boolean {
  if (!record || now >= record.expiresAt) return false;
  return record.count >= cap;
}

export type RateLimitScope = "email" | "ip" | "global";

export interface RateLimitDecision {
  allowed: boolean;
  scope?: RateLimitScope;
}

/**
 * Evaluate all three caps against counters already read from KV. Checked
 * before any counter is incremented — a request that trips a cap must not
 * also consume budget from the caps it didn't trip.
 */
export function evaluateRateLimits(params: {
  emailRecord: CounterRecord | null;
  ipRecord: CounterRecord | null;
  globalRecord: CounterRecord | null;
  now: number;
  caps: { email: number; ip: number; global: number };
}): RateLimitDecision {
  const { emailRecord, ipRecord, globalRecord, now, caps } = params;
  if (isOverCap(emailRecord, now, caps.email)) return { allowed: false, scope: "email" };
  if (isOverCap(ipRecord, now, caps.ip)) return { allowed: false, scope: "ip" };
  if (isOverCap(globalRecord, now, caps.global)) return { allowed: false, scope: "global" };
  return { allowed: true };
}
