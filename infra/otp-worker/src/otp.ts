//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
/**
 * OTP code generation, hashing, and the verify-attempt state machine. This
 * is the single most security-sensitive module in the Worker — a 6-digit
 * code is trivially brute-forced offline from a raw KV dump, which is why
 * `code:<hash>` never stores the plain code (see {@link hashCode}) and why
 * {@link verifyOtpAttempt} is written as a pure function: the one invariant
 * it exists to protect — a wrong guess must NEVER extend the record's
 * remaining TTL — is exactly the kind of thing that's easy to regress inside
 * a KV read/write handler and easy to pin with a unit test outside one.
 *
 * # Who calls this
 * - `index.ts`'s `/otp/send` handler (`generateCode`, `hashCode`,
 *   `createOtpRecord`)
 * - `index.ts`'s `/otp/verify` handler (`hashCode`, `verifyOtpAttempt`)
 *
 * # Related
 * - `crypto-utils.ts` — `hmacSha256Hex` (the actual HMAC), `timingSafeEqualStrings`
 * - `ratelimit.ts` — the sibling KV-record state machine for send caps,
 *   built on the same "pure decision, thin KV glue" split
 */

import { hmacSha256Hex, timingSafeEqualStrings } from "./crypto-utils";

/** The `code:<hash>` KV value shape. */
export interface OtpRecord {
  /** HMAC-SHA256(pepper, code) — never the bare code. */
  codeHash: string;
  attempts: number;
  /** Epoch ms. Authoritative expiry check — KV's own TTL is best-effort cleanup only. */
  expiresAt: number;
}

/**
 * Cryptographically random 6-digit code via rejection sampling — avoids the
 * modulo-bias `byte % 10` would introduce (256 is not a multiple of 10, so a
 * plain modulo maps bytes 0-5 to digit 0-5 with a fractionally higher chance
 * than 6-9; discarding bytes 250-255 removes that bias entirely). Per
 * `workers-best-practices`: never `Math.random()` for anything
 * security-sensitive.
 */
export function generateCode(length = 6): string {
  const digits: string[] = [];
  const buf = new Uint8Array(1);
  while (digits.length < length) {
    crypto.getRandomValues(buf);
    const byte = buf[0] as number;
    if (byte >= 250) continue; // 250-255 discarded: 256 % 10 !== 0
    digits.push(String(byte % 10));
  }
  return digits.join("");
}

/** HMAC-SHA256(pepper, code), hex-encoded — the only form of the code that touches KV. */
export async function hashCode(code: string, pepper: string): Promise<string> {
  return hmacSha256Hex(pepper, code);
}

/** Build a fresh record for a newly-sent code. `ttlMs` comes from `vars.OTP_TTL_S * 1000`. */
export function createOtpRecord(codeHash: string, now: number, ttlMs: number): OtpRecord {
  return { codeHash, attempts: 0, expiresAt: now + ttlMs };
}

export type VerifyOutcome =
  | { kind: "verified" }
  | { kind: "wrong"; nextRecord: OtpRecord; attemptsRemaining: number }
  | { kind: "exhausted" }
  | { kind: "not_found_or_expired" };

/**
 * Apply one verify attempt against the current record. Pure: takes the
 * record read from KV and returns what to write back (or `null` via the
 * `verified`/`exhausted`/`not_found_or_expired` kinds, all of which mean
 * "the caller deletes the KV key").
 *
 * `exhausted` and `not_found_or_expired` are deliberately the SAME outward
 * HTTP response (410, see `responses.ts`) — a caller must not be able to
 * distinguish "you used up your 5 attempts" from "there was never a live
 * code for this email" from the HTTP layer alone; both just mean "request a
 * new code".
 */
export function verifyOtpAttempt(
  record: OtpRecord | null,
  providedCodeHash: string,
  now: number,
  maxAttempts: number,
): VerifyOutcome {
  if (!record) return { kind: "not_found_or_expired" };
  if (now >= record.expiresAt) return { kind: "not_found_or_expired" };
  if (record.attempts >= maxAttempts) return { kind: "exhausted" };

  if (timingSafeEqualStrings(providedCodeHash, record.codeHash)) {
    return { kind: "verified" };
  }

  const nextAttempts = record.attempts + 1;
  if (nextAttempts >= maxAttempts) {
    return { kind: "exhausted" };
  }

  // Preserve `expiresAt` EXACTLY — a wrong guess must never extend the
  // record's remaining TTL. This line is the one this module exists to get
  // right; see `otp.test.ts`'s `wrong guess never extends expiresAt` case.
  return {
    kind: "wrong",
    nextRecord: { codeHash: record.codeHash, attempts: nextAttempts, expiresAt: record.expiresAt },
    attemptsRemaining: maxAttempts - nextAttempts,
  };
}
