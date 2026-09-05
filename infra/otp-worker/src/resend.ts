//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
/**
 * The "someone signed up / changed their email" notification to the team,
 * delivered via **Resend** — deliberately a different provider to the OTP
 * codes themselves, which stay on SES (`ses.ts`).
 *
 * # Why this one is not on SES
 * The marketing site has sent this exact notification since June via Resend
 * (`Meridian Sign-ins <notify@meridiona.com>` → the team inbox, subject
 * `New sign-up: <email>`). Routing the desktop app's copy through the same
 * provider keeps web and desktop sign-ups in one inbox, one dashboard and one
 * searchable history, with one sender identity, rather than splitting them
 * across two providers by accident of which codebase emitted them.
 *
 * This does NOT reverse the SES-over-Resend decision recorded in
 * `README.md` — that decision was specifically about OTP *code* delivery,
 * where Resend's free tier (100/day) cannot cover the expected hundreds of
 * user-facing sends a day. An internal notification to a single address is a
 * couple of dozen a day at most, so the volume objection simply does not
 * apply to it.
 *
 * # Who calls this
 * - `index.ts`'s `/otp/verify` handler, on the `verified` outcome only, via
 *   `ctx.waitUntil` (fire-and-forget — see {@link sendAccountEventEmail}).
 *
 * # Related
 * - `ses.ts` — OTP code delivery and the rate-limit alert, both still SES.
 * - README.md — "Account-event notification".
 */

/** Resend's transactional send endpoint. */
const RESEND_ENDPOINT = "https://api.resend.com/emails";

export interface ResendEnv {
  RESEND_API_KEY: string;
  /** Full RFC 5322 from-header, e.g. `Meridian Sign-ins <notify@meridiona.com>`. */
  NOTIFY_FROM: string;
  /** Single internal recipient. Absent/empty disables the notification entirely. */
  NOTIFY_EMAIL: string;
}

/**
 * The `fetch` slice this module uses, so tests can intercept the network call
 * without a live API key. Mirrors `ses.ts`'s `AwsFetcher` rationale.
 */
export type Fetcher = (input: string, init: RequestInit) => Promise<Response>;

/**
 * `previousEmail` is client-supplied and purely informational (see
 * `index.ts`'s `handleVerify`) — never used for a security decision, only to
 * word this notification.
 */
export type AccountEvent =
  | { kind: "signed_up"; email: string }
  | { kind: "email_changed"; from: string; to: string };

/**
 * Decide what (if anything) happened, from `handleVerify`'s point of view.
 * Pure, so it is unit-tested directly rather than only through a full
 * send→verify round trip.
 *
 * `previousEmail` arrives already normalized by `email.ts`'s
 * `normalizeEmail`, or `null` when absent/unparseable. Returns `null` for a
 * no-op re-verify of the SAME address — which happens legitimately when
 * "Change email" is used to re-enter the address already on file, and must
 * not generate a notification saying nothing changed.
 */
export function resolveAccountEvent(
  newEmail: string,
  previousEmail: string | null,
): AccountEvent | null {
  if (previousEmail === newEmail) return null;
  if (previousEmail) return { kind: "email_changed", from: previousEmail, to: newEmail };
  return { kind: "signed_up", email: newEmail };
}

/**
 * Subject line, matching the marketing site's existing convention exactly
 * (`New sign-up: <email>`) so both sources thread together in the inbox.
 * `email_changed` has no web equivalent and gets its own prefix.
 */
export function buildAccountEventSubject(event: AccountEvent): string {
  return event.kind === "signed_up"
    ? `New sign-up: ${event.email}`
    : `Email changed: ${event.from} -> ${event.to}`;
}

/**
 * Plain-text body, deliberately mirroring the shape the website already
 * sends: the address on line 1, an identifying line, then one sentence of
 * status. **Text only, no HTML part** — the web notification has no HTML
 * part either, and an internal one-line alert gains nothing from markup.
 *
 * Where the web version carries `Clerk user id: …`, the desktop app has no
 * equivalent (Clerk was removed), so the second line names the source
 * instead — which is also what makes a desktop notification distinguishable
 * from a web one at a glance.
 */
export function buildAccountEventBody(event: AccountEvent): string {
  const source = "Source: desktop app (email OTP)";
  return event.kind === "signed_up"
    ? `${event.email}\n${source}\nFirst time signing in.\n`
    : `${event.to}\n${source}\nChanged from ${event.from}.\n`;
}

/**
 * Send the notification. Returns `false` (never throws) on any failure —
 * network error, non-2xx, or an unset API key — because this rides on
 * `ctx.waitUntil` behind an OTP verify that has already succeeded from the
 * user's point of view. A failed notification must never turn into a failed
 * sign-in.
 *
 * The API key is checked here rather than at the call site so a Worker
 * deployed without the secret degrades to "no notification" instead of
 * throwing inside a `waitUntil` where nothing would surface it.
 */
export async function sendAccountEventEmail(
  event: AccountEvent,
  env: ResendEnv,
  fetcher: Fetcher = fetch,
): Promise<boolean> {
  if (!env.RESEND_API_KEY) {
    console.error("otp-worker: RESEND_API_KEY is not configured — account-event notification skipped");
    return false;
  }
  try {
    const res = await fetcher(RESEND_ENDPOINT, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${env.RESEND_API_KEY}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({
        from: env.NOTIFY_FROM,
        to: [env.NOTIFY_EMAIL],
        subject: buildAccountEventSubject(event),
        text: buildAccountEventBody(event),
      }),
    });
    if (!res.ok) {
      // Deliberately logs the STATUS only, never the response body: a Resend
      // error echoes the `to`/`from` addresses back, and the recipient here
      // is an internal address we have no reason to put in Workers Logs.
      console.error("otp-worker: account-event notification failed", {
        status: res.status,
        kind: event.kind,
      });
      return false;
    }
    return true;
  } catch (err) {
    console.error("otp-worker: account-event notification threw", { error: String(err) });
    return false;
  }
}
