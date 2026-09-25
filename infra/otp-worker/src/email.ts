//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
/**
 * Email normalization and the KV-key hash. Raw email addresses are NEVER
 * used as a KV key directly — always {@link emailHash} of the
 * {@link normalizeEmail}-d form, per the plan's KV schema.
 *
 * # Related
 * - `crypto-utils.ts` — the underlying `sha256Hex`
 * - `index.ts` — the only caller
 */

import { sha256Hex } from "./crypto-utils";

/** RFC 5321 hard upper bound on a full email address. */
const MAX_EMAIL_LENGTH = 320;

/**
 * Normalize a raw email for hashing/delivery: trim, lowercase, and a
 * deliberately permissive syntactic check.
 *
 * Full RFC 5322 validation is not this Worker's job — SES will bounce
 * anything it can't deliver. This only needs to reject obviously-malformed
 * input before it becomes a KV key or an SES recipient.
 *
 * Returns `null` for anything that doesn't look like an email at all.
 */
export function normalizeEmail(raw: unknown): string | null {
  if (typeof raw !== "string") return null;
  const trimmed = raw.trim().toLowerCase();
  if (trimmed.length === 0 || trimmed.length > MAX_EMAIL_LENGTH) return null;
  if (!/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(trimmed)) return null;
  return trimmed;
}

/** sha256 hex digest of a normalized email — the KV key material. */
export async function emailHash(normalizedEmail: string): Promise<string> {
  return sha256Hex(normalizedEmail);
}
