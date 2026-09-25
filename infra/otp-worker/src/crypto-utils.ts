//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
/**
 * Small crypto primitives shared by {@link "./auth"}, {@link "./otp"} and
 * {@link "./email"}. Split out on its own so none of those three modules
 * import from each other just to reach a hex-encode helper.
 *
 * # Related
 * - `auth.ts` — bearer-token comparison
 * - `otp.ts` — OTP code hashing/verification
 * - `email.ts` — email-address hashing for KV keys
 */

/** Hex-encode a digest/signature buffer. */
export function bufferToHex(buf: ArrayBuffer): string {
  return Array.from(new Uint8Array(buf))
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

/**
 * Constant-time string comparison, guarding both a bearer token check and an
 * OTP-code-hash check against timing side-channels (see
 * `workers-best-practices`: "Direct string comparison for secret values").
 *
 * Cloudflare's `crypto.subtle.timingSafeEqual` requires equal-length inputs
 * and throws otherwise, so an unequal-length pair returns `false` up front
 * without calling it. That is a length-based timing signal in principle, but
 * length alone is not the secret here (a bearer token's length isn't
 * sensitive, and OTP-code HMACs are always the same fixed digest length) —
 * only the content comparison needs to be constant-time, which is what this
 * still guarantees for any two equal-length inputs.
 */
export function timingSafeEqualStrings(a: string, b: string): boolean {
  const enc = new TextEncoder();
  const aBytes = enc.encode(a);
  const bBytes = enc.encode(b);
  if (aBytes.byteLength !== bBytes.byteLength) {
    return false;
  }
  return crypto.subtle.timingSafeEqual(aBytes, bBytes);
}

/** SHA-256 hex digest of a UTF-8 string — used for the KV email-hash key. */
export async function sha256Hex(input: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(input));
  return bufferToHex(digest);
}

/** HMAC-SHA256 hex digest of `message` under `key` — used for OTP code hashing. */
export async function hmacSha256Hex(key: string, message: string): Promise<string> {
  const cryptoKey = await crypto.subtle.importKey(
    "raw",
    new TextEncoder().encode(key),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
  const sig = await crypto.subtle.sign("HMAC", cryptoKey, new TextEncoder().encode(message));
  return bufferToHex(sig);
}
