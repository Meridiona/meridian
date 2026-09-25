//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
/**
 * Thin `KVNamespace` read/write glue for `OTP_KV`. Deliberately the only
 * module that touches the binding directly — `otp.ts` and `ratelimit.ts`
 * stay pure and unit-testable, this just serializes their record shapes to
 * and from JSON with the right TTL.
 *
 * Every record embeds its own authoritative `expiresAt` (epoch ms) rather
 * than relying solely on KV's own TTL, for two reasons documented on
 * `otp.ts`/`ratelimit.ts`: (1) a rolling window's expiry must survive a
 * read-modify-write without being reset on every increment, which KV's
 * `expirationTtl` alone can't do (it must be re-specified on every `put`);
 * and (2) **Cloudflare Workers KV requires `expirationTtl >= 60`** — a
 * record with, say, 20 real seconds left cannot be written back with a
 * 20-second KV TTL. {@link ttlSecondsFromExpiry} clamps to that floor, which
 * only affects the KV-level storage cleanup timer; `verifyOtpAttempt` and
 * `isOverCap`'s own `now >= expiresAt` checks are what actually enforce
 * expiry, so a clamped KV TTL can never let an expired record be honoured —
 * it can only make the dead key sit in storage a little longer before KV
 * itself reaps it.
 *
 * # Who calls this
 * - `index.ts`'s `/otp/send` and `/otp/verify` handlers.
 *
 * # Related
 * - `otp.ts` — `OtpRecord`, the pure verify state machine
 * - `ratelimit.ts` — `CounterRecord`, the pure cap-decision logic
 */

import type { OtpRecord } from "./otp";
import type { CounterRecord } from "./ratelimit";

/** Cloudflare Workers KV's hard minimum for `expirationTtl`, in seconds. */
const KV_MIN_TTL_S = 60;

/** Seconds until `expiresAt`, clamped to KV's minimum TTL. See module header. */
export function ttlSecondsFromExpiry(expiresAt: number, now: number): number {
  return Math.max(KV_MIN_TTL_S, Math.ceil((expiresAt - now) / 1000));
}

function otpKey(emailHash: string): string {
  return `code:${emailHash}`;
}

export async function getOtpRecord(kv: KVNamespace, emailHashValue: string): Promise<OtpRecord | null> {
  return kv.get<OtpRecord>(otpKey(emailHashValue), "json");
}

export async function putOtpRecord(
  kv: KVNamespace,
  emailHashValue: string,
  record: OtpRecord,
  now: number,
): Promise<void> {
  await kv.put(otpKey(emailHashValue), JSON.stringify(record), {
    expirationTtl: ttlSecondsFromExpiry(record.expiresAt, now),
  });
}

export async function deleteOtpRecord(kv: KVNamespace, emailHashValue: string): Promise<void> {
  await kv.delete(otpKey(emailHashValue));
}

export async function getCounter(kv: KVNamespace, key: string): Promise<CounterRecord | null> {
  return kv.get<CounterRecord>(key, "json");
}

/**
 * Write a counter record. `ttlOverrideS`, when given, is used verbatim
 * instead of the derived clamp — used for `global:sends:<date>`, whose KV
 * TTL is a fixed 90000s cleanup buffer per the plan (deliberately longer
 * than one calendar day) rather than something derived from the embedded
 * window, since that key's real "window" is just "this UTC date".
 */
export async function putCounter(
  kv: KVNamespace,
  key: string,
  record: CounterRecord,
  now: number,
  ttlOverrideS?: number,
): Promise<void> {
  await kv.put(key, JSON.stringify(record), {
    expirationTtl: ttlOverrideS ?? ttlSecondsFromExpiry(record.expiresAt, now),
  });
}

function alertSentKey(date: string): string {
  return `alert:sent:${date}`;
}

/** Whether the daily rate-limit-approaching alert has already fired for this
 *  UTC date — makes the alert a once-per-day event rather than firing again
 *  on every request once the threshold is crossed. */
export async function hasAlertBeenSent(kv: KVNamespace, date: string): Promise<boolean> {
  return (await kv.get(alertSentKey(date))) !== null;
}

/** Record that the alert fired for this UTC date. `ttlS` mirrors the global
 *  counter's own cleanup buffer (see `index.ts`'s `GLOBAL_COUNTER_KV_TTL_S`)
 *  — deliberately longer than one day, not a precise window. */
export async function markAlertSent(kv: KVNamespace, date: string, ttlS: number): Promise<void> {
  await kv.put(alertSentKey(date), "1", { expirationTtl: ttlS });
}
