//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
/**
 * Pure rate-limit decision logic, kept separate from the Durable Object
 * storage glue in `rate-limiter.ts` so the caps themselves are
 * unit-testable without a simulated DO.
 *
 * Three independent caps, checked in this order (cheapest/most-specific
 * first): per-email, per-IP, then global — see `index.ts`'s `handleCapture`.
 * The email and IP counters are rolling fixed windows that open on the
 * first capture and reset only once that window's `expiresAt` has passed —
 * NOT a calendar-boundary reset — which is why the expiry is carried in the
 * value itself. The global counter is namespaced by UTC date instead (see
 * {@link utcDateString}), so its "window" is just "this key exists for one
 * calendar day".
 *
 * # Who calls this
 * - `rate-limiter.ts`'s `RateLimitCounter.tryConsume`, inside the Durable
 *   Object turn that makes these checks atomic.
 */

/** A rolling fixed-window counter — one per Durable Object instance. */
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
