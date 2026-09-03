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
    /** HMAC pepper for OTP code hashing — see `otp.ts`. Never the bare code. */
    OTP_CODE_PEPPER: string;
    AWS_ACCESS_KEY_ID: string;
    AWS_SECRET_ACCESS_KEY: string;
    AWS_REGION: string;
    /**
     * Staging-only bearer token that also unlocks the `/otp/send` code-echo
     * (see `auth.ts`). Never set outside `env.staging` — its mere presence on
     * production would still be inert there (see `auth.ts`'s `ENVIRONMENT`
     * check), but it should never be set there in the first place.
     */
    CI_TEST_TOKEN?: string;
    /**
     * Cloudflare Turnstile secret key. Unset until/unless the frontend
     * feasibility spike (see plan) lands and a Turnstile site is
     * provisioned — see `turnstile.ts` for the unconfigured-secret behaviour.
     */
    TURNSTILE_SECRET_KEY?: string;
  }
}
