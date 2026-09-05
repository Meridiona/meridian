//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, expect, it } from "vitest";
import { evaluateRateLimits, incrementCounter, isOverCap, shouldSendRateLimitAlert, utcDateString } from "../ratelimit";

describe("incrementCounter", () => {
  it("opens a fresh window (count 1) when there is no existing record", () => {
    const now = 1_000_000;
    const windowMs = 3600_000;
    expect(incrementCounter(null, now, windowMs)).toEqual({ count: 1, expiresAt: now + windowMs });
  });

  it("opens a fresh window when the existing one has expired", () => {
    const now = 1_000_000;
    const windowMs = 3600_000;
    const expired = { count: 9, expiresAt: now - 1 };
    expect(incrementCounter(expired, now, windowMs)).toEqual({ count: 1, expiresAt: now + windowMs });
  });

  it("increments in place and preserves expiresAt within an open window", () => {
    const now = 1_000_000;
    const existing = { count: 2, expiresAt: now + 500_000 };
    const next = incrementCounter(existing, now, 3600_000);
    expect(next).toEqual({ count: 3, expiresAt: existing.expiresAt });
  });
});

describe("isOverCap", () => {
  it("is false for a null record", () => {
    expect(isOverCap(null, 1000, 3)).toBe(false);
  });

  it("is false for an expired record regardless of count", () => {
    expect(isOverCap({ count: 999, expiresAt: 999 }, 1000, 3)).toBe(false);
  });

  it("is false below the cap", () => {
    expect(isOverCap({ count: 2, expiresAt: 2000 }, 1000, 3)).toBe(false);
  });

  it("is true at exactly the cap", () => {
    expect(isOverCap({ count: 3, expiresAt: 2000 }, 1000, 3)).toBe(true);
  });

  it("is true above the cap", () => {
    expect(isOverCap({ count: 10, expiresAt: 2000 }, 1000, 3)).toBe(true);
  });
});

describe("evaluateRateLimits", () => {
  const now = 1000;
  const caps = { email: 3, ip: 10, global: 2000 };
  const underCap = { count: 1, expiresAt: 2000 };
  // Comfortably exceeds every cap in `caps` above (email:3, ip:10, global:2000).
  const overCap = { count: 100_000, expiresAt: 2000 };

  it("allows when all three counters are under cap", () => {
    const decision = evaluateRateLimits({ emailRecord: underCap, ipRecord: underCap, globalRecord: underCap, now, caps });
    expect(decision).toEqual({ allowed: true });
  });

  it("allows when all three counters are absent", () => {
    const decision = evaluateRateLimits({ emailRecord: null, ipRecord: null, globalRecord: null, now, caps });
    expect(decision).toEqual({ allowed: true });
  });

  it("denies with scope 'email' when only the email cap is tripped, checked first", () => {
    const decision = evaluateRateLimits({ emailRecord: overCap, ipRecord: overCap, globalRecord: overCap, now, caps });
    expect(decision).toEqual({ allowed: false, scope: "email" });
  });

  it("denies with scope 'ip' when only the ip cap is tripped", () => {
    const decision = evaluateRateLimits({ emailRecord: underCap, ipRecord: overCap, globalRecord: underCap, now, caps });
    expect(decision).toEqual({ allowed: false, scope: "ip" });
  });

  it("denies with scope 'global' when only the global cap is tripped", () => {
    const decision = evaluateRateLimits({ emailRecord: underCap, ipRecord: underCap, globalRecord: overCap, now, caps });
    expect(decision).toEqual({ allowed: false, scope: "global" });
  });
});

describe("utcDateString", () => {
  it("formats as YYYY-MM-DD in UTC", () => {
    // 2026-03-05T23:30:00Z
    const ms = Date.UTC(2026, 2, 5, 23, 30, 0);
    expect(utcDateString(ms)).toBe("2026-03-05");
  });
});

describe("shouldSendRateLimitAlert", () => {
  it("does not alert below the threshold", () => {
    expect(shouldSendRateLimitAlert(1599, 2000, 80, false)).toBe(false);
  });

  it("alerts exactly at the threshold", () => {
    expect(shouldSendRateLimitAlert(1600, 2000, 80, false)).toBe(true);
  });

  it("alerts above the threshold too", () => {
    expect(shouldSendRateLimitAlert(2000, 2000, 80, false)).toBe(true);
  });

  it("never alerts twice in the same day, regardless of count", () => {
    expect(shouldSendRateLimitAlert(2000, 2000, 80, true)).toBe(false);
  });

  it("is disabled when thresholdPct is 0 or negative — not 'alert on every send'", () => {
    expect(shouldSendRateLimitAlert(2000, 2000, 0, false)).toBe(false);
    expect(shouldSendRateLimitAlert(2000, 2000, -5, false)).toBe(false);
  });

  it("is disabled when cap is non-positive (misconfiguration, not a division trap)", () => {
    expect(shouldSendRateLimitAlert(10, 0, 80, false)).toBe(false);
  });
});
