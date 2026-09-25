//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, expect, it } from "vitest";
import { createOtpRecord, generateCode, hashCode, verifyOtpAttempt, type OtpRecord } from "../otp";

describe("generateCode", () => {
  it("produces a 6-digit numeric string by default", () => {
    const code = generateCode();
    expect(code).toMatch(/^\d{6}$/);
  });

  it("produces different codes across calls (not a fixed value)", () => {
    const codes = new Set(Array.from({ length: 20 }, () => generateCode()));
    expect(codes.size).toBeGreaterThan(1);
  });

  it("honours a custom length", () => {
    expect(generateCode(4)).toMatch(/^\d{4}$/);
  });
});

describe("hashCode / createOtpRecord", () => {
  it("hashCode is deterministic for the same pepper and code", async () => {
    const a = await hashCode("123456", "pepper");
    const b = await hashCode("123456", "pepper");
    expect(a).toBe(b);
  });

  it("createOtpRecord starts at zero attempts with the given expiry", () => {
    const record = createOtpRecord("somehash", 1000, 600_000);
    expect(record).toEqual({ codeHash: "somehash", attempts: 0, expiresAt: 601_000 });
  });
});

describe("verifyOtpAttempt", () => {
  const NOW = 1_000_000;
  const MAX_ATTEMPTS = 5;

  async function makeRecord(code: string, expiresAt: number, attempts = 0): Promise<OtpRecord> {
    return { codeHash: await hashCode(code, "pepper"), attempts, expiresAt };
  }

  it("verifies a correct code", async () => {
    const record = await makeRecord("123456", NOW + 60_000);
    const providedHash = await hashCode("123456", "pepper");
    const outcome = verifyOtpAttempt(record, providedHash, NOW, MAX_ATTEMPTS);
    expect(outcome.kind).toBe("verified");
  });

  it("returns not_found_or_expired for a null record", () => {
    const outcome = verifyOtpAttempt(null, "any-hash", NOW, MAX_ATTEMPTS);
    expect(outcome.kind).toBe("not_found_or_expired");
  });

  it("returns not_found_or_expired once now has reached expiresAt", async () => {
    const record = await makeRecord("123456", NOW); // expires exactly at NOW
    const providedHash = await hashCode("123456", "pepper");
    const outcome = verifyOtpAttempt(record, providedHash, NOW, MAX_ATTEMPTS);
    expect(outcome.kind).toBe("not_found_or_expired");
  });

  it("a wrong guess increments attempts but returns the SAME expiresAt — never extends the TTL", async () => {
    const record = await makeRecord("123456", NOW + 60_000, 0);
    const wrongHash = await hashCode("000000", "pepper");
    const outcome = verifyOtpAttempt(record, wrongHash, NOW, MAX_ATTEMPTS);
    expect(outcome.kind).toBe("wrong");
    if (outcome.kind !== "wrong") throw new Error("unreachable");
    expect(outcome.nextRecord.expiresAt).toBe(record.expiresAt);
    expect(outcome.nextRecord.attempts).toBe(1);
    expect(outcome.nextRecord.codeHash).toBe(record.codeHash);
    expect(outcome.attemptsRemaining).toBe(MAX_ATTEMPTS - 1);
  });

  it("the exact 5th wrong guess (maxAttempts reached) exhausts the record instead of returning wrong", async () => {
    const record = await makeRecord("123456", NOW + 60_000, MAX_ATTEMPTS - 1); // one guess left
    const wrongHash = await hashCode("000000", "pepper");
    const outcome = verifyOtpAttempt(record, wrongHash, NOW, MAX_ATTEMPTS);
    expect(outcome.kind).toBe("exhausted");
  });

  it("a record that already reached maxAttempts is exhausted even before this call's comparison", async () => {
    const record = await makeRecord("123456", NOW + 60_000, MAX_ATTEMPTS);
    const providedHash = await hashCode("123456", "pepper"); // even the RIGHT code
    const outcome = verifyOtpAttempt(record, providedHash, NOW, MAX_ATTEMPTS);
    expect(outcome.kind).toBe("exhausted");
  });

  it("four consecutive wrong guesses each preserve expiresAt, the fifth exhausts", async () => {
    let record = await makeRecord("123456", NOW + 60_000, 0);
    const wrongHash = await hashCode("000000", "pepper");
    const originalExpiry = record.expiresAt;

    for (let i = 0; i < MAX_ATTEMPTS - 1; i++) {
      const outcome = verifyOtpAttempt(record, wrongHash, NOW, MAX_ATTEMPTS);
      expect(outcome.kind).toBe("wrong");
      if (outcome.kind !== "wrong") throw new Error("unreachable");
      expect(outcome.nextRecord.expiresAt).toBe(originalExpiry);
      record = outcome.nextRecord;
    }
    expect(record.attempts).toBe(MAX_ATTEMPTS - 1);

    const finalOutcome = verifyOtpAttempt(record, wrongHash, NOW, MAX_ATTEMPTS);
    expect(finalOutcome.kind).toBe("exhausted");
  });
});
