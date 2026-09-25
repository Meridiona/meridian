//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
/**
 * Optional server-side Turnstile verification for `/otp/send`.
 *
 * Turnstile support is wired but CONDITIONAL, per the plan: whether the
 * client actually sends a `turnstileToken` depends on a separate
 * frontend/Tauri feasibility spike this Worker's implementation does not
 * block on (see README.md's "Turnstile" section for the full reasoning and
 * the resolved edge case below).
 *
 * Two independent gates, both must be satisfied for verification to run:
 *  1. The request body actually included a `turnstileToken` — if absent,
 *     `index.ts` never calls this module at all and the send proceeds on
 *     bearer-auth + rate-limits alone (the plan's explicit "if absent,
 *     proceed without it").
 *  2. `env.TURNSTILE_SECRET_KEY` is actually configured — if it's unset
 *     (the expected state until/unless the frontend spike succeeds and a
 *     site is provisioned), verification is a no-op: {@link verifyTurnstileToken}
 *     returns `true` and logs a warning, rather than rejecting every send
 *     because a feature nothing has enabled yet is "misconfigured". Once a
 *     secret IS configured, gate 1 is what actually matters day to day.
 *
 * Resolved (not specified by the plan): a token that IS present but FAILS
 * verification is rejected (403) rather than silently ignored — fail closed
 * on invalid input, consistent with this repo's conventions elsewhere. A
 * present-and-valid token verifies as usual; an absent token skips this
 * module entirely (per gate 1 above); an unconfigured secret with a present
 * token also proceeds (per gate 2), since without a secret there's nothing
 * to validate against.
 *
 * # Who calls this
 * - `index.ts`'s `/otp/send` handler, only when `turnstileToken` is present
 *   in the request body.
 *
 * # Related
 * - README.md — the Turnstile spike status and the secret name
 *   (`TURNSTILE_SECRET_KEY`) is documented there since it's not one of the
 *   plan's originally-listed secrets.
 */

const SITEVERIFY_URL = "https://challenges.cloudflare.com/turnstile/v0/siteverify";

interface SiteverifyResponse {
  success?: boolean;
  "error-codes"?: string[];
}

/**
 * Verify a Turnstile token via Cloudflare's siteverify API.
 *
 * Fails closed on any network error, non-2xx response, or unparseable body —
 * an inability to verify is treated the same as a failed verification, never
 * as an implicit pass.
 */
export async function verifyTurnstileToken(
  token: string,
  secretKey: string | undefined,
  remoteIp: string | undefined,
  fetchImpl: typeof fetch = fetch,
): Promise<boolean> {
  if (!secretKey || secretKey.length === 0) {
    // Gate 2: feature not provisioned yet — see module header.
    console.warn("otp-worker: turnstileToken present but TURNSTILE_SECRET_KEY unset — skipping verification");
    return true;
  }

  const body = new URLSearchParams();
  body.set("secret", secretKey);
  body.set("response", token);
  if (remoteIp) body.set("remoteip", remoteIp);

  try {
    const res = await fetchImpl(SITEVERIFY_URL, { method: "POST", body });
    if (!res.ok) {
      console.warn("otp-worker: turnstile siteverify non-2xx", { status: res.status });
      return false;
    }
    const data = (await res.json()) as SiteverifyResponse;
    if (data.success !== true) {
      console.warn("otp-worker: turnstile siteverify rejected", { errorCodes: data["error-codes"] ?? [] });
      return false;
    }
    return true;
  } catch (err) {
    console.error("otp-worker: turnstile siteverify request failed", { error: String(err) });
    return false;
  }
}
