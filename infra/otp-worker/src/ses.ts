//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
/**
 * Email delivery via AWS SES's `SendEmail` (Query API, v1,
 * `2010-12-01`), SigV4-signed with `aws4fetch` — Workers run on a V8
 * isolate with no Node.js APIs, so the official AWS SDK doesn't work here;
 * `aws4fetch` is the established community pattern for calling AWS from a
 * Worker. Resolved (not specified by the plan): SES v1's Query API over
 * SESv2's JSON API, because it's the one with a documented `aws4fetch`
 * example (`service: "email"`, form-urlencoded body) — this is a low-stakes
 * choice per a single `SendEmail` call either way.
 *
 * `from` is always `${FROM_NAME} <${FROM_ADDRESS}>` where `FROM_ADDRESS` is
 * a verified subdomain of meridiona.com (`vars.FROM_ADDRESS` in
 * `wrangler.jsonc`, currently the placeholder `otp@auth.meridiona.com` —
 * DNS verification is a manual step outside this Worker's code, see
 * README.md). Plain code, no links, per the plan.
 *
 * # Who calls this
 * - `index.ts`'s `/otp/send` handler, after the OTP record is written and
 *   all rate-limit/Turnstile gates have passed.
 *
 * # Related
 * - README.md — the SES-vs-Resend rationale and the sandbox/production-access
 *   prerequisite (external, not something this code can detect at runtime
 *   beyond a failed send surfacing as a 503).
 */

import { AwsClient } from "aws4fetch";

export interface SesEnv {
  AWS_ACCESS_KEY_ID: string;
  AWS_SECRET_ACCESS_KEY: string;
  AWS_REGION: string;
  FROM_ADDRESS: string;
  FROM_NAME: string;
}

/**
 * The slice of `AwsClient` this module actually uses. Tests inject a fake
 * implementing just this shape instead of a real `AwsClient` — `AwsClient`
 * has no `fetch` override in its `RequestInit` (only an `aws` signing-options
 * override), so the only way to intercept the network call for a test is at
 * this level, not by passing a custom `fetch` down into `client.fetch()`.
 */
export interface AwsFetcher {
  fetch(input: string, init?: RequestInit): Promise<Response>;
}

/** The exact body sent to the user — no links, matching the plan. */
export function buildOtpEmailBody(code: string, ttlMinutes: number): string {
  return (
    `Your Meridian verification code is: ${code}\n\n` +
    `This code expires in ${ttlMinutes} minutes. If you didn't request this, you can safely ignore this email.`
  );
}

/**
 * Extract a safe, loggable identifier from an SES error response WITHOUT
 * ever logging the raw body — SES's sandbox-mode "email address is not
 * verified" error echoes the destination address back in the response text,
 * which must never reach Workers Logs verbatim.
 */
export function extractSesErrorCode(responseText: string): string {
  try {
    const parsed = JSON.parse(responseText) as { Error?: { Code?: string }; message?: string };
    return parsed.Error?.Code ?? parsed.message ?? "unknown_ses_error_shape";
  } catch {
    return "unparseable_ses_error_response";
  }
}

/**
 * Send the OTP email. Returns `false` (never throws) on any failure so the
 * caller can map it to a 503 without a try/catch of its own — network
 * errors, non-2xx SES responses, and thrown exceptions are all "delivery
 * failed" from the caller's point of view.
 */
export async function sendOtpEmail(
  toEmail: string,
  code: string,
  ttlMinutes: number,
  env: SesEnv,
  client: AwsFetcher = new AwsClient({
    accessKeyId: env.AWS_ACCESS_KEY_ID,
    secretAccessKey: env.AWS_SECRET_ACCESS_KEY,
    service: "email",
    region: env.AWS_REGION,
  }),
): Promise<boolean> {
  const body = new URLSearchParams({
    Action: "SendEmail",
    Version: "2010-12-01",
    Source: `${env.FROM_NAME} <${env.FROM_ADDRESS}>`,
    "Destination.ToAddresses.member.1": toEmail,
    "Message.Subject.Data": "Your Meridian verification code",
    "Message.Subject.Charset": "UTF-8",
    "Message.Body.Text.Data": buildOtpEmailBody(code, ttlMinutes),
    "Message.Body.Text.Charset": "UTF-8",
  });

  try {
    const res = await client.fetch(`https://email.${env.AWS_REGION}.amazonaws.com/`, {
      method: "POST",
      headers: {
        "Content-Type": "application/x-www-form-urlencoded",
        Accept: "application/json",
      },
      body: body.toString(),
    });
    if (!res.ok) {
      const text = await res.text();
      console.error("otp-worker: ses send failed", { status: res.status, errorCode: extractSesErrorCode(text) });
      return false;
    }
    return true;
  } catch (err) {
    console.error("otp-worker: ses send threw", { error: String(err) });
    return false;
  }
}
