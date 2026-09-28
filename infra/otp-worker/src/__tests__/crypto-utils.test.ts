//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, expect, it } from "vitest";
import { sha256Hex, timingSafeEqualStrings } from "../crypto-utils";

describe("timingSafeEqualStrings", () => {
  it("returns true for identical strings", () => {
    expect(timingSafeEqualStrings("abc123", "abc123")).toBe(true);
  });

  it("returns false for different strings of the same length", () => {
    expect(timingSafeEqualStrings("abc123", "abc124")).toBe(false);
  });

  it("returns false for different-length strings without throwing", () => {
    expect(timingSafeEqualStrings("short", "a-lot-longer-string")).toBe(false);
  });

  it("returns false comparing against an empty string", () => {
    expect(timingSafeEqualStrings("nonempty", "")).toBe(false);
  });

  it("treats two empty strings as equal", () => {
    expect(timingSafeEqualStrings("", "")).toBe(true);
  });
});

describe("sha256Hex", () => {
  it("is deterministic for the same input", async () => {
    const a = await sha256Hex("test@example.com");
    const b = await sha256Hex("test@example.com");
    expect(a).toBe(b);
    expect(a).toMatch(/^[0-9a-f]{64}$/);
  });

  it("differs for different input", async () => {
    const a = await sha256Hex("a@example.com");
    const b = await sha256Hex("b@example.com");
    expect(a).not.toBe(b);
  });
});
