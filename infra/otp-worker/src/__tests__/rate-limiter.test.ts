//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
/**
 * Integration-style tests against the real (Miniflare-simulated)
 * `RATE_LIMITER` Durable Object binding — no hand-rolled fake, so a real
 * storage/serialization mistake in `rate-limiter.ts` would actually be
 * caught here, the same way `kv.test.ts` used to catch KV serialization
 * mistakes before this Worker moved off KV for its counters.
 *
 * The concurrency test below is the actual regression test for the bug this
 * module exists to fix — see `rate-limiter.ts`'s module header for the
 * TOCTOU race a plain KV read/write had.
 */
import { env } from "cloudflare:test";
import { describe, expect, it } from "vitest";

/** A fresh, uniquely-keyed stub per test so tests never share counter state. */
function stubFor(key: string) {
  return env.RATE_LIMITER.get(env.RATE_LIMITER.idFromName(key));
}

describe("RateLimitCounter.tryConsume", () => {
  it("allows requests under the cap", async () => {
    const stub = stubFor("allows-under-cap");
    const now = Date.now();
    expect(await stub.tryConsume(now, 3_600_000, 3)).toBe(true);
    expect(await stub.tryConsume(now, 3_600_000, 3)).toBe(true);
    expect(await stub.tryConsume(now, 3_600_000, 3)).toBe(true);
  });

  it("denies once the cap is reached, and stays denied on further attempts", async () => {
    const stub = stubFor("denies-at-cap");
    const now = Date.now();
    expect(await stub.tryConsume(now, 3_600_000, 2)).toBe(true);
    expect(await stub.tryConsume(now, 3_600_000, 2)).toBe(true);
    expect(await stub.tryConsume(now, 3_600_000, 2)).toBe(false);
    expect(await stub.tryConsume(now, 3_600_000, 2)).toBe(false);
  });

  it("opens a fresh window once the previous one has expired", async () => {
    const stub = stubFor("fresh-window-after-expiry");
    const start = Date.now();
    const windowMs = 1_000;
    expect(await stub.tryConsume(start, windowMs, 1)).toBe(true);
    expect(await stub.tryConsume(start, windowMs, 1)).toBe(false); // still in-window, over cap
    expect(await stub.tryConsume(start + windowMs + 1, windowMs, 1)).toBe(true); // window expired
  });

  it("different keys route to different instances and never share budget", async () => {
    const now = Date.now();
    const a = stubFor("distinct-key-a");
    const b = stubFor("distinct-key-b");
    expect(await a.tryConsume(now, 3_600_000, 1)).toBe(true);
    expect(await a.tryConsume(now, 3_600_000, 1)).toBe(false); // a is now at cap
    expect(await b.tryConsume(now, 3_600_000, 1)).toBe(true); // b is untouched by a's usage
  });

  /**
   * **The regression test for the TOCTOU race this module replaces.**
   *
   * The old KV-based implementation read a counter, evaluated it against the
   * cap, and wrote back an incremented value as two separate calls — under
   * concurrency, multiple requests could read the same under-cap count and
   * all "win", letting the cap be exceeded. Firing many concurrent
   * `tryConsume` calls at the SAME counter must let through EXACTLY `cap` of
   * them, never more — if Durable Objects' automatic concurrency control
   * (see `rate-limiter.ts`'s module header) were ever bypassed or removed,
   * this is what would catch it: `allowedCount` would drift above `cap`.
   */
  it("serializes concurrent calls against the same counter — never exceeds the cap", async () => {
    const stub = stubFor("concurrency-never-exceeds-cap");
    const now = Date.now();
    const cap = 5;
    const attempts = 20;

    const results = await Promise.all(
      Array.from({ length: attempts }, () => stub.tryConsume(now, 3_600_000, cap)),
    );

    const allowedCount = results.filter((allowed) => allowed).length;
    expect(allowedCount).toBe(cap);
  });
});
