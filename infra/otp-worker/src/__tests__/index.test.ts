//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
/**
 * Wiring-level tests for `index.ts`'s router and auth gate — every gate
 * module (`auth.ts`, `otp.ts`, `ratelimit.ts`, `turnstile.ts`) has its own
 * thorough unit tests, but until this file nothing ever exercised the actual
 * exported `fetch` handler that composes them. That left CLAUDE.md's Hard
 * Rules #1 ("authenticate every request") and #3 ("allowlist the paths it
 * serves") — the two properties this Worker exists specifically to get
 * right, per README.md's "Why this design" — asserted by NOTHING: the
 * deploy script's mock-server exercise (see scripts/deploy-otp-worker.sh)
 * only ever tests the SCRIPT, not this Worker.
 *
 * Deliberately scoped to what needs no secrets: there is no `.dev.vars` here
 * (and shouldn't be — that would mean committing a bearer token, even a fake
 * one, next to code path this repo is specifically careful about), so
 * `OTP_CLIENT_TOKEN` is unset in this simulated environment. Per `auth.ts`,
 * an empty/unset configured token never matches ANY provided token — which
 * means these cases require zero setup AND incidentally re-confirm the
 * empty-secret-never-passes guarantee from `auth.test.ts` one layer up, at
 * the real `fetch` handler rather than the isolated `checkBearerAuth` call.
 *
 * # Related
 * - `index.ts` — the handler under test
 * - `scripts/deploy-otp-worker.sh` — the live-deploy counterpart of these
 *   same 401/404 assertions, run against a real deployed Worker
 */
import { SELF } from "cloudflare:test";
import { describe, expect, it } from "vitest";

describe("router: only POST /otp/send and POST /otp/verify exist", () => {
  it("404s a GET to /otp/send — a method mismatch is not the allowlisted route", async () => {
    const res = await SELF.fetch("https://example.com/otp/send");
    expect(res.status).toBe(404);
    expect(await res.json()).toEqual({ error: "not_found" });
  });

  it("404s POST to an unknown path", async () => {
    const res = await SELF.fetch("https://example.com/otp/unknown", { method: "POST" });
    expect(res.status).toBe(404);
  });

  it("404s POST to the bare root", async () => {
    const res = await SELF.fetch("https://example.com/", { method: "POST" });
    expect(res.status).toBe(404);
  });
});

describe("auth gate: unauthenticated requests never reach KV/SES", () => {
  it("401s POST /otp/send with no Authorization header", async () => {
    const res = await SELF.fetch("https://example.com/otp/send", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ email: "test@example.com" }),
    });
    expect(res.status).toBe(401);
    expect(await res.json()).toEqual({ error: "unauthorized" });
  });

  it("401s POST /otp/verify with no Authorization header", async () => {
    const res = await SELF.fetch("https://example.com/otp/verify", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ email: "test@example.com", code: "123456" }),
    });
    expect(res.status).toBe(401);
  });

  it("401s POST /otp/send with a bearer token, since no OTP_CLIENT_TOKEN secret is configured here", async () => {
    // This is the empty-secret-never-passes guarantee (auth.test.ts) proven
    // at the real handler: an unconfigured secret must reject EVERY token,
    // not just requests with none at all.
    const res = await SELF.fetch("https://example.com/otp/send", {
      method: "POST",
      headers: { "Content-Type": "application/json", Authorization: "Bearer anything-at-all" },
      body: JSON.stringify({ email: "test@example.com" }),
    });
    expect(res.status).toBe(401);
  });

  it("auth is checked before body parsing — malformed JSON with no auth still 401s, not 400", async () => {
    const res = await SELF.fetch("https://example.com/otp/send", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: "not json at all {{{",
    });
    expect(res.status).toBe(401);
  });
});
