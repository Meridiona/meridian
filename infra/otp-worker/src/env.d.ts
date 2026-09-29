//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
/**
 * Augments the generated `Env` interface (`worker-configuration.d.ts`, from
 * `npm run types` / `wrangler types`) with the secrets that only ever exist
 * via `wrangler secret put` — `wrangler types` has no way to see these since
 * they're never written to `wrangler.jsonc`.
 *
 * Deliberately NOT using a hand-written `Env` interface for everything
 * (`workers-best-practices`' anti-pattern list flags exactly that): the
 * bindings/vars half of `Env` still comes from the generated file, this only
 * adds the secret-shaped half on top via declaration merging.
 *
 * # Related
 * - `worker-configuration.d.ts` — generated, not committed by hand; run
 *   `npm run types` after any `wrangler.jsonc` binding/vars change.
 */

export {};

declare global {
  interface Env {
    /** Bearer token the tray binary sends — see `auth.ts`. */
    OTP_CLIENT_TOKEN: string;
    /**
     * Resend sending key for the account-event notification (sign-up /
     * email changed) — see `resend.ts`. Scoped in the Resend dashboard to
     * sending-access on the `meridiona.com` domain, so it cannot manage the
     * account or send as `mail.meridiona.com`.
     */
    RESEND_API_KEY: string;
  }
}
