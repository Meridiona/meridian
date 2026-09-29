//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, expect, it } from "vitest";
import { incrementCounter, isOverCap, utcDateString } from "../ratelimit";

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

describe("utcDateString", () => {
  it("formats as YYYY-MM-DD in UTC", () => {
    // 2026-03-05T23:30:00Z
    const ms = Date.UTC(2026, 2, 5, 23, 30, 0);
    expect(utcDateString(ms)).toBe("2026-03-05");
  });
});
