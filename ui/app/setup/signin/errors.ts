//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

// Clerk error-shape helpers, shared by every Clerk call site in `signin/`.
//
// `create()`/`sendCode()`/`verifyCode()`/`finalize()` resolve as `{ error }` —
// `error` is a `ClerkAPIResponseError` whose OWN `.message` is just a generic
// wrapper string and which has no `.code` at all; the actual per-field
// code/message Clerk wants you to branch on lives in `error.errors[0]`. Every
// read of a Clerk error's code or text must go through these two helpers —
// reading `.code`/`.message` straight off `error` (the bug this replaced)
// meant a `form_identifier_not_found` check silently never matched, so the
// sign-up fallback never fired and Clerk's raw per-field text ("Couldn't
// find your account.") leaked to the screen as a dead end instead of
// starting sign-up.

export type ClerkErrorLike = {
  errors?: { code: string; message: string; longMessage?: string }[]
  message?: string
} | null | undefined

export function clerkErrorCode(e: ClerkErrorLike): string | undefined {
  return e?.errors?.[0]?.code
}

export function clerkErrorMessage(e: ClerkErrorLike, fallback: string): string {
  const first = e?.errors?.[0]
  return first?.longMessage || first?.message || e?.message || fallback
}
