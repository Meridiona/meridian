//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { env } from "cloudflare:test";
import { beforeEach, describe, expect, it } from "vitest";
import { deleteOtpRecord, getCounter, getOtpRecord, putCounter, putOtpRecord, ttlSecondsFromExpiry } from "../kv";

describe("ttlSecondsFromExpiry", () => {
  it("computes the ceiling of the remaining seconds", () => {
    expect(ttlSecondsFromExpiry(1_000_000 + 90_500, 1_000_000)).toBe(91);
  });

  it("clamps to KV's 60s minimum when less than 60s remain", () => {
    expect(ttlSecondsFromExpiry(1_000_000 + 5_000, 1_000_000)).toBe(60);
  });

  it("clamps to 60s even when the record is already expired (negative remaining)", () => {
    expect(ttlSecondsFromExpiry(1_000_000 - 5_000, 1_000_000)).toBe(60);
  });
});

// Integration-style tests against the real (Miniflare-simulated) OTP_KV
// binding declared in wrangler.jsonc — no network, no Cloudflare account,
// but exercises the actual KVNamespace.get/put/delete round trip rather than
// a hand-rolled fake, so a real serialization or TTL-argument mistake in
// kv.ts would actually be caught here.
describe("OtpRecord KV round-trip", () => {
  const HASH = "deadbeef".repeat(8); // stand-in for a real sha256 hex hash

  beforeEach(async () => {
    await deleteOtpRecord(env.OTP_KV, HASH);
  });

  it("returns null for a key that was never written", async () => {
    expect(await getOtpRecord(env.OTP_KV, HASH)).toBeNull();
  });

  it("round-trips a written record byte-for-byte", async () => {
    const now = Date.now();
    const record = { codeHash: "somehash", attempts: 2, expiresAt: now + 600_000 };
    await putOtpRecord(env.OTP_KV, HASH, record, now);
    expect(await getOtpRecord(env.OTP_KV, HASH)).toEqual(record);
  });

  it("delete removes the record", async () => {
    const now = Date.now();
    await putOtpRecord(env.OTP_KV, HASH, { codeHash: "x", attempts: 0, expiresAt: now + 600_000 }, now);
    await deleteOtpRecord(env.OTP_KV, HASH);
    expect(await getOtpRecord(env.OTP_KV, HASH)).toBeNull();
  });
});

describe("CounterRecord KV round-trip", () => {
  const KEY = "rl:email:test-key";

  it("round-trips a counter record", async () => {
    const now = Date.now();
    const record = { count: 2, expiresAt: now + 3_600_000 };
    await putCounter(env.OTP_KV, KEY, record, now);
    expect(await getCounter(env.OTP_KV, KEY)).toEqual(record);
  });

  it("honours a ttlOverrideS without changing the stored record shape", async () => {
    const now = Date.now();
    const record = { count: 1, expiresAt: now + 86_400_000 };
    await putCounter(env.OTP_KV, "global:sends:test-date", record, now, 90_000);
    expect(await getCounter(env.OTP_KV, "global:sends:test-date")).toEqual(record);
  });
});
