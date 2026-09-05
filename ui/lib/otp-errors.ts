//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

// Maps the error sentinel strings `request_account_otp`/`confirm_account_otp`
// (tray/src-tauri/src/commands/otp.rs) reject with to the exact copy
// `OtpForm.tsx` shows. Kept as pure, exported functions so the mapping is
// unit-tested directly — this codebase's test suite has no DOM/React
// rendering harness (bun test only), so a component's *behavior* is only
// testable through logic it delegates to a plain function like this one.
//
// Tauri rejects a `Result<_, String>` command with the raw string, not an
// Error object — `errorCode` handles that shape defensively (a thrown
// network-transport failure could still arrive as a real `Error`).

const GENERIC_ERROR = 'Something went wrong - check your connection and try again.'
const NOT_CONFIGURED_MESSAGE =
  "OTP isn't configured in this build - set OTP_API_URL in your .env to test sign-in."

export type OtpSendOutcome = {
  message: string
  /** True only for the fresh-clone dev case (`OTP_API_URL` unset/blank) — the
   *  wizard step should let the user continue anyway instead of leaving Next
   *  permanently disabled. See `steps.tsx`'s `EMAIL_STEP.canNext`. */
  isDevBypass: boolean
}

function errorCode(err: unknown): string {
  if (typeof err === 'string') return err
  if (err instanceof Error) return err.message
  return ''
}

/** Maps `request_account_otp`'s rejection to send-phase copy. */
export function classifySendError(err: unknown): OtpSendOutcome {
  switch (errorCode(err)) {
    case 'invalid_email':
      return { message: "That doesn't look like a valid email address.", isDevBypass: false }
    case 'rate_limited':
      return { message: 'Too many codes requested - try again later.', isDevBypass: false }
    case 'unavailable':
      return { message: 'Sign-in is temporarily unavailable - try again in a moment.', isDevBypass: false }
    case 'unauthorized':
      return { message: GENERIC_ERROR, isDevBypass: false }
    case 'blocked':
      return { message: "That request couldn't be verified - try again.", isDevBypass: false }
    case 'not_configured':
      return { message: NOT_CONFIGURED_MESSAGE, isDevBypass: true }
    default:
      return { message: GENERIC_ERROR, isDevBypass: false }
  }
}

/** Maps `confirm_account_otp`'s rejection to verify-phase copy. A wrong (but
 *  well-formed) code is never a rejection — the command resolves `false` for
 *  that, handled separately by the caller. `expired` covers BOTH "the code
 *  expired or was never sent" AND "you used up your attempts" - the Worker
 *  deliberately returns the same signal for both
 *  (`infra/otp-worker/src/otp.ts`'s `VerifyOutcome` doc), so this can't and
 *  shouldn't try to show a distinct "too many attempts" message. */
export function classifyVerifyError(err: unknown): { message: string; isDevBypass: boolean } {
  switch (errorCode(err)) {
    case 'expired':
      return { message: 'That code no longer works - request a new one.', isDevBypass: false }
    case 'invalid_input':
      return { message: "That code didn't work - check it and try again.", isDevBypass: false }
    case 'rate_limited':
      return { message: 'Too many codes requested - try again later.', isDevBypass: false }
    case 'unavailable':
      return { message: 'Sign-in is temporarily unavailable - try again in a moment.', isDevBypass: false }
    case 'unauthorized':
      return { message: GENERIC_ERROR, isDevBypass: false }
    case 'not_configured':
      return { message: NOT_CONFIGURED_MESSAGE, isDevBypass: true }
    default:
      return { message: GENERIC_ERROR, isDevBypass: false }
  }
}
