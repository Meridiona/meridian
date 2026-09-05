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

/** The exact plain-text body sent to the user — no links, matching the plan.
 *  Kept as the `Text` part alongside {@link buildOtpEmailHtml}'s `Html` part
 *  so clients that don't render HTML (or have it disabled) still get a
 *  readable code. */
export function buildOtpEmailBody(code: string, ttlMinutes: number): string {
  return (
    `Your Meridian verification code is: ${code}\n\n` +
    `This code expires in ${ttlMinutes} minutes. If you didn't request this, you can safely ignore this email.`
  );
}

/** Escape the handful of characters that matter inside an HTML text node.
 *  `code` is always exactly 6 digits (see `otp.ts`'s `generateCode`) and
 *  never needs this, but `ttlMinutes` is attacker-uncontrolled server config,
 *  not user input — this exists as defense-in-depth if either shape ever
 *  changes, not because either is untrusted today. */
function escapeHtml(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

/**
 * The HTML body sent alongside the plain-text part. No `<script>`, no
 * `onclick` — email clients strip all JavaScript from message bodies
 * unconditionally (Gmail, Outlook, Apple Mail alike), so a real
 * click-to-copy button is not achievable inside an email; every major
 * product with this same OTP-by-email pattern (Google, GitHub, Stripe)
 * solves it the same way this does instead: render the code large, bold,
 * monospaced, letter-spaced, and alone in its own box, so a single click
 * (most clients auto-select a triple-clicked or double-clicked isolated
 * token) or one drag-select copies exactly the code and nothing else.
 *
 * Table-based layout with every style attribute inlined, because Outlook's
 * rendering engine (Word, not a browser engine) and stripped-down webmail
 * CSS parsers are the two things that break `<div>`+`<style>`-block emails
 * most often; this shape survives both. No external assets (images, web
 * fonts, or CSS) are loaded, so there is nothing for a client's image-proxy
 * or content-security policy to block or delay.
 */
export function buildOtpEmailHtml(code: string, ttlMinutes: number): string {
  const safeCode = escapeHtml(code);
  const safeTtl = escapeHtml(String(ttlMinutes));
  return `<!doctype html>
<html>
  <body style="margin:0;padding:0;background-color:#f4f4f5;">
    <table role="presentation" width="100%" cellpadding="0" cellspacing="0" style="background-color:#f4f4f5;padding:32px 16px;">
      <tr>
        <td align="center">
          <table role="presentation" width="100%" cellpadding="0" cellspacing="0" style="max-width:480px;background-color:#ffffff;border-radius:12px;overflow:hidden;font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;">
            <tr>
              <td style="padding:32px 32px 8px 32px;">
                <p style="margin:0;font-size:15px;line-height:22px;color:#18181b;">Your verification code for Meridian:</p>
              </td>
            </tr>
            <tr>
              <td style="padding:16px 32px;">
                <table role="presentation" width="100%" cellpadding="0" cellspacing="0" style="background-color:#f4f4f5;border-radius:10px;">
                  <tr>
                    <td align="center" style="padding:22px 16px;">
                      <span style="font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;font-size:34px;font-weight:700;letter-spacing:10px;color:#18181b;">${safeCode}</span>
                    </td>
                  </tr>
                </table>
              </td>
            </tr>
            <tr>
              <td style="padding:8px 32px 32px 32px;">
                <p style="margin:0;font-size:13px;line-height:20px;color:#71717a;">This code expires in ${safeTtl} minutes. If you didn't request this, you can safely ignore this email.</p>
              </td>
            </tr>
          </table>
        </td>
      </tr>
    </table>
  </body>
</html>`;
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

function defaultAwsClient(env: SesEnv): AwsFetcher {
  return new AwsClient({
    accessKeyId: env.AWS_ACCESS_KEY_ID,
    secretAccessKey: env.AWS_SECRET_ACCESS_KEY,
    service: "email",
    region: env.AWS_REGION,
  });
}

/**
 * Shared low-level "POST one email via SES's Query API" primitive —
 * {@link sendOtpEmail} and {@link sendRateLimitAlertEmail} both build a
 * subject/text/html triple and hand it here rather than duplicating the
 * SigV4 request-building and error-handling shape. Returns `false` (never
 * throws) on any failure, same contract both callers already relied on.
 */
async function postSesEmail(
  toEmail: string,
  subject: string,
  textBody: string,
  htmlBody: string,
  env: SesEnv,
  client: AwsFetcher,
  logContext: string,
): Promise<boolean> {
  const body = new URLSearchParams({
    Action: "SendEmail",
    Version: "2010-12-01",
    Source: `${env.FROM_NAME} <${env.FROM_ADDRESS}>`,
    "Destination.ToAddresses.member.1": toEmail,
    "Message.Subject.Data": subject,
    "Message.Subject.Charset": "UTF-8",
    "Message.Body.Text.Data": textBody,
    "Message.Body.Text.Charset": "UTF-8",
    "Message.Body.Html.Data": htmlBody,
    "Message.Body.Html.Charset": "UTF-8",
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
      console.error(`otp-worker: ${logContext} send failed`, {
        status: res.status,
        errorCode: extractSesErrorCode(text),
      });
      return false;
    }
    return true;
  } catch (err) {
    console.error(`otp-worker: ${logContext} send threw`, { error: String(err) });
    return false;
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
  client: AwsFetcher = defaultAwsClient(env),
): Promise<boolean> {
  return postSesEmail(
    toEmail,
    "Your Meridian verification code",
    buildOtpEmailBody(code, ttlMinutes),
    buildOtpEmailHtml(code, ttlMinutes),
    env,
    client,
    "ses",
  );
}

/** The plain-text body of the rate-limit-approaching alert email. */
export function buildRateLimitAlertBody(currentCount: number, cap: number, thresholdPct: number): string {
  return (
    `The OTP Worker's global daily send count has reached ${currentCount}/${cap} ` +
    `(${thresholdPct}% threshold) for today (UTC).\n\n` +
    `This is a heads-up, not an outage: sends continue normally until the cap is reached, ` +
    `at which point new codes are rejected with a 429 until the daily window resets at 00:00 UTC. ` +
    `If this volume is expected, raise RL_GLOBAL_PER_DAY in wrangler.jsonc and redeploy; ` +
    `if it isn't, this may be worth investigating for abuse.`
  );
}

/** The HTML body of the rate-limit-approaching alert — same plain, no-links
 *  shape as the OTP email's HTML part, just different copy. */
export function buildRateLimitAlertHtml(currentCount: number, cap: number, thresholdPct: number): string {
  const safeBody = escapeHtml(buildRateLimitAlertBody(currentCount, cap, thresholdPct)).replace(/\n\n/g, "</p><p>");
  return `<!doctype html>
<html>
  <body style="margin:0;padding:0;background-color:#f4f4f5;">
    <table role="presentation" width="100%" cellpadding="0" cellspacing="0" style="background-color:#f4f4f5;padding:32px 16px;">
      <tr>
        <td align="center">
          <table role="presentation" width="100%" cellpadding="0" cellspacing="0" style="max-width:480px;background-color:#ffffff;border-radius:12px;overflow:hidden;font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;">
            <tr>
              <td style="padding:28px 32px;">
                <p style="margin:0 0 8px 0;font-size:15px;font-weight:600;color:#18181b;">OTP Worker approaching its daily send limit</p>
                <p style="margin:0;font-size:13px;line-height:20px;color:#3f3f46;">${safeBody}</p>
              </td>
            </tr>
          </table>
        </td>
      </tr>
    </table>
  </body>
</html>`;
}

/**
 * Fire the once-per-day "approaching the global daily cap" alert. Returns
 * `false` (never throws) on failure, same contract as `sendOtpEmail` — a
 * failed alert send must never affect the OTP send it rode in on (see
 * `index.ts`'s `ctx.waitUntil` call site, which doesn't await this inline).
 */
export async function sendRateLimitAlertEmail(
  currentCount: number,
  cap: number,
  thresholdPct: number,
  env: SesEnv & { ALERT_EMAIL: string },
  client: AwsFetcher = defaultAwsClient(env),
): Promise<boolean> {
  return postSesEmail(
    env.ALERT_EMAIL,
    "Meridian OTP Worker: approaching daily send limit",
    buildRateLimitAlertBody(currentCount, cap, thresholdPct),
    buildRateLimitAlertHtml(currentCount, cap, thresholdPct),
    env,
    client,
    "rate-limit alert",
  );
}
