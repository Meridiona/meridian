//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The plan surface's shared control styles. Extracted from PlanView so the columns and
// the composer share one definition instead of three drifting copies.
//
// On the colours (STYLESHEET §9's three-hue budget): PRIMARY is GREEN
// (--color-state-approved) because every primary here is a COMMIT — Confirm, Save,
// Create. Violet (--color-state-proposal) is reserved for AI-authored/proposed things
// (the Draft button, the "Drafted" chips, the Suggested chip). Don't swap them: a
// violet Confirm would read as "this is a proposal", which is the opposite of what
// the button does.

/** Keyboard focus ring. Every interactive element in the plan gets this. */
export const FOCUS =
  'outline-none focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2'

/** A commit action — pair with `background: var(--color-state-approved)` + white text. */
export const PRIMARY_BTN =
  'mt-body-sm px-4 py-2 rounded-lg font-semibold transition-opacity hover:opacity-90'

/** A secondary action — pair with a `--t-ctrl-border` border + `--t-muted` text. */
export const GHOST_BTN = 'mt-body-sm px-4 py-2 rounded-lg bg-ctrl transition-opacity hover:opacity-80'
