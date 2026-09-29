//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, expect, it } from "vitest";
import { emailHash, normalizeEmail } from "../email";

describe("normalizeEmail", () => {
  it("trims and lowercases a valid address", () => {
    expect(normalizeEmail("  Test@Example.COM  ")).toBe("test@example.com");
  });

  it("rejects non-string input", () => {
    expect(normalizeEmail(undefined)).toBeNull();
    expect(normalizeEmail(null)).toBeNull();
    expect(normalizeEmail(12345)).toBeNull();
    expect(normalizeEmail({})).toBeNull();
  });

  it("rejects an empty or whitespace-only string", () => {
    expect(normalizeEmail("")).toBeNull();
    expect(normalizeEmail("   ")).toBeNull();
  });

  it("rejects a string with no @", () => {
    expect(normalizeEmail("not-an-email")).toBeNull();
  });

  it("rejects a string with no domain dot", () => {
    expect(normalizeEmail("a@b")).toBeNull();
  });

  it("rejects an address longer than the RFC 5321 bound", () => {
    const longLocal = "a".repeat(310);
    expect(normalizeEmail(`${longLocal}@example.com`)).toBeNull();
  });

  it("rejects a string containing whitespace inside the address", () => {
    expect(normalizeEmail("a b@example.com")).toBeNull();
  });
});

describe("emailHash", () => {
  it("is deterministic and hex-encoded", async () => {
    const a = await emailHash("test@example.com");
    const b = await emailHash("test@example.com");
    expect(a).toBe(b);
    expect(a).toMatch(/^[0-9a-f]{64}$/);
  });

  it("differs for a different normalized email", async () => {
    const a = await emailHash("a@example.com");
    const b = await emailHash("b@example.com");
    expect(a).not.toBe(b);
  });
});
