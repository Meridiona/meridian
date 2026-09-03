//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, expect, it } from "vitest";
import { hmacSha256Hex, sha256Hex, timingSafeEqualStrings } from "../crypto-utils";

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

describe("hmacSha256Hex", () => {
  it("is deterministic for the same key and message", async () => {
    const a = await hmacSha256Hex("pepper", "123456");
    const b = await hmacSha256Hex("pepper", "123456");
    expect(a).toBe(b);
    expect(a).toMatch(/^[0-9a-f]{64}$/);
  });

  it("differs when the pepper differs — a code hash from one pepper must not verify under another", async () => {
    const a = await hmacSha256Hex("pepper-one", "123456");
    const b = await hmacSha256Hex("pepper-two", "123456");
    expect(a).not.toBe(b);
  });

  it("differs when the code differs", async () => {
    const a = await hmacSha256Hex("pepper", "123456");
    const b = await hmacSha256Hex("pepper", "654321");
    expect(a).not.toBe(b);
  });
});
