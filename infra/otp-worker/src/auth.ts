//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
/**
 * Bearer-token origin auth — proves "a genuine Meridian binary sent this",
 * not "this is a human". Mirrors the pattern already in production at
 * `tray/src-tauri/src/counter_ping.rs` (compiled-in default, bearer auth),
 * ported to the Worker side of that same handshake. Documented honestly per
 * the plan: this is attestation, not a strong secret — the token is
 * extractable from the shipped tray binary. Rate limiting (`ratelimit.ts`)
 * and, optionally, Turnstile (`turnstile.ts`) are the actual abuse
 * containment; this only keeps out casual/opportunistic callers who never
 * had a Meridian binary at all.
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
  /** Staging-only, see below — absent in production. */
  CI_TEST_TOKEN?: string;
  ENVIRONMENT?: string;
}

export interface AuthResult {
  ok: boolean;
  /**
   * True only when the request authenticated with the staging-only
   * `CI_TEST_TOKEN` rather than the normal `OTP_CLIENT_TOKEN`. This is what
   * gates the staging code-echo in `/otp/send` — never true outside
   * `env.ENVIRONMENT === "staging"`, checked again explicitly below rather
   * than relying solely on the secret being unset in production (the plan's
   * "never reachable with the production bearer token, enforced with an
   * explicit `env.ENVIRONMENT !== "staging"` guard, not just an unset var").
   */
  isCiTestToken: boolean;
}

const DENY: AuthResult = { ok: false, isCiTestToken: false };

function extractBearerToken(header: string | null): string | null {
  if (!header) return null;
  const match = /^Bearer\s+(.+)$/.exec(header);
  return match?.[1] ?? null;
}

/**
 * Check the `Authorization` header against the configured client token (and,
 * on staging only, the CI test token). An unconfigured/empty secret never
 * matches an empty or missing token — guards against the classic "both
 * sides blank" bypass if a secret was never set.
 */
export function checkBearerAuth(authorizationHeader: string | null, env: AuthEnv): AuthResult {
  const token = extractBearerToken(authorizationHeader);
  if (!token) return DENY;

  const clientToken = env.OTP_CLIENT_TOKEN ?? "";
  if (clientToken.length > 0 && timingSafeEqualStrings(token, clientToken)) {
    return { ok: true, isCiTestToken: false };
  }

  if (env.ENVIRONMENT === "staging") {
    const ciToken = env.CI_TEST_TOKEN ?? "";
    if (ciToken.length > 0 && timingSafeEqualStrings(token, ciToken)) {
      return { ok: true, isCiTestToken: true };
    }
  }

  return DENY;
}
