//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
/**
 * Bearer-token origin auth — proves "a genuine Meridian binary sent this",
 * not "this is a human". Mirrors the pattern already in production at
 * `tray/src-tauri/src/counter_ping.rs` (compiled-in default, bearer auth),
 * ported to the Worker side of that same handshake. Documented honestly per
 * the plan: this is attestation, not a strong secret — the token is
 * extractable from the shipped tray binary. Rate limiting (`ratelimit.ts`)
 * is the actual abuse containment; this only keeps out casual/opportunistic
 * callers who never had a Meridian binary at all.
 *
 * # Who calls this
 * - `index.ts`, on every request, before any body parsing or KV access.
 *
 * # Related
 * - `crypto-utils.ts` — the timing-safe comparison this relies on.
 */

import { timingSafeEqualStrings } from "./crypto-utils";

export interface AuthEnv {
  OTP_CLIENT_TOKEN?: string;
}

export interface AuthResult {
  ok: boolean;
}

const DENY: AuthResult = { ok: false };

function extractBearerToken(header: string | null): string | null {
  if (!header) return null;
  const match = /^Bearer\s+(.+)$/.exec(header);
  return match?.[1] ?? null;
}

/**
 * Check the `Authorization` header against the configured client token. An
 * unconfigured/empty secret never matches an empty or missing token — guards
 * against the classic "both sides blank" bypass if a secret was never set.
 */
export function checkBearerAuth(authorizationHeader: string | null, env: AuthEnv): AuthResult {
  const token = extractBearerToken(authorizationHeader);
  if (!token) return DENY;

  const clientToken = env.OTP_CLIENT_TOKEN ?? "";
  if (clientToken.length > 0 && timingSafeEqualStrings(token, clientToken)) {
    return { ok: true };
  }

  return DENY;
}
