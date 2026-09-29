//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
/**
 * A Durable Object holding exactly one rolling-window counter (one per
 * email hash / IP / UTC date — see `index.ts`'s `handleCapture` for how the
 * three keys are derived and routed here via `idFromName`).
 *
 * # Why a Durable Object, not plain KV
 * Cloudflare Workers KV has no atomic increment or compare-and-swap. The
 * previous implementation read a counter, evaluated it against the cap in
 * the Worker, and wrote back an incremented value as two separate KV calls —
 * a classic TOCTOU race: two concurrent requests can both read the same
 * under-cap count, both decide "allowed", and both write back the same
 * incremented value, so the cap is silently bypassed under concurrency (this
 * was flagged in a security review of the original `/otp/send` code, which
 * `/otp/capture` inherited unchanged).
 *
 * A Durable Object closes this: Cloudflare guarantees a single DO instance
 * processes one request's JavaScript at a time, and by default blocks a
 * second concurrent request from starting until any in-flight
 * `ctx.storage` operation on the first has completed (the "input/output
 * gate" — see the Durable Objects docs on automatic concurrency control).
 * {@link RateLimitCounter.tryConsume} does its read-check-write entirely
 * within one such gated turn, so two concurrent calls against the SAME
 * counter (same `idFromName` key) are strictly serialized: the second
 * always observes the first's write before making its own decision. Calls
 * against DIFFERENT keys (different emails, IPs, dates) hit different DO
 * instances and still run fully in parallel — the gate only serializes
 * access to one counter, never the Worker as a whole.
 *
 * # Storage cleanup
 * KV's `expirationTtl` used to reap expired counters for free. A DO's own
 * storage has no such passive TTL, so a counter that is never revisited
 * again would otherwise sit in storage forever. {@link alarm} deletes the
 * counter once its window has passed, scheduled from {@link tryConsume}
 * every time it writes — this mirrors the old KV TTL's cleanup role, not its
 * correctness role (a stale-but-not-yet-reaped record is already harmless:
 * `isOverCap`/`incrementCounter` both treat `now >= expiresAt` as "expired",
 * regardless of whether the alarm has run yet).
 *
 * # Who calls this
 * - `index.ts`'s `handleCapture`, once per cap (email, ip, global), in that
 *   order, stopping at the first `tryConsume` that returns `false`.
 *
 * # Related
 * - `ratelimit.ts` — the pure `CounterRecord`/`incrementCounter`/`isOverCap`
 *   this wraps with durable storage.
 */

import { DurableObject } from "cloudflare:workers";
import { incrementCounter, isOverCap, type CounterRecord } from "./ratelimit";

/** Buffer past `expiresAt` before the cleanup alarm fires — purely cosmetic
 *  (see the module header: an unreaped record is never incorrectly honoured
 *  as still-live), gives a request that lands right at the boundary no
 *  chance to race the alarm's delete. */
const ALARM_BUFFER_MS = 5_000;

const STORAGE_KEY = "counter";

export class RateLimitCounter extends DurableObject {
  /**
   * Atomically: is this counter currently at or past `cap`? If not,
   * increment it (opening a fresh window if the previous one expired) and
   * return `true`; if so, leave it untouched and return `false`.
   *
   * No `await` sits between the `storage.get` and the `storage.put` other
   * than the get/put calls themselves, and Durable Objects' automatic
   * concurrency control blocks a second invocation of this method (or any
   * other one touching `ctx.storage`) from starting while either of those is
   * in flight — that is the entire fix. See the module header.
   */
  async tryConsume(now: number, windowMs: number, cap: number): Promise<boolean> {
    const existing = (await this.ctx.storage.get<CounterRecord>(STORAGE_KEY)) ?? null;
    if (isOverCap(existing, now, cap)) return false;

    const next = incrementCounter(existing, now, windowMs);
    await this.ctx.storage.put(STORAGE_KEY, next);
    await this.ctx.storage.setAlarm(next.expiresAt + ALARM_BUFFER_MS);
    return true;
  }

  /** Cleanup: drop the counter once its window has passed. See module header. */
  async alarm(): Promise<void> {
    await this.ctx.storage.delete(STORAGE_KEY);
  }
}
