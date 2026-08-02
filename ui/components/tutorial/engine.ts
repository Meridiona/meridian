//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

// The walkthrough's step primitives — a React-side port of the four mechanics the
// public marketing demo already uses (meridiona-website/assets/js/demo.js):
// an animated cursor, a narration caption, a camera that zooms onto whatever is
// being explained, and `waitForUserClick` — the one that matters most, because it
// is what makes this a walkthrough the user DOES rather than a slideshow they
// watch.
//
// The script is written as a plain async function against a `Stage` handle, so
// each beat reads in the order it happens (narrate → point → hand over → react)
// instead of being scattered across a step-index reducer. That is the same shape
// demo.js's `runSummaryDemo` uses, and it is why the copy and pacing can be
// carried across nearly unchanged.
//
// # Who calls this
// [`useTutorial`] builds the `Stage` and runs `runScript` against it.
//
// # Related
// - `./sampleDay.ts` — the example day the script narrates over
// - `./TutorialOverlay.tsx` — renders the cursor/caption/spotlight this drives

import type { SettingsSection } from '@/components/timeline/settings/types'

/** Everything a script beat can do to the running app. Implemented by
 *  [`useTutorial`]; kept as an interface so the script has no React in it and
 *  can be read top-to-bottom as pure narrative. */
export interface Stage {
  /** Set the narration line. Empty string clears it. */
  say(text: string): void
  /** Move the animated cursor over the first element matching `selector`.
   *  A miss is a no-op, never a throw — a beat pointing at a control that a
   *  given user's state does not render (no tracker connected, say) should
   *  degrade to narration rather than break the run. */
  point(selector: string): Promise<void>
  /** Play the cursor's click animation where it currently sits. */
  click(): void
  /** Highlight the element matching `selector`, or clear it with `null`.
   *
   *  Two strengths. The default is a RING only: the rest of the screen stays
   *  sharp and usable, which is right for beats explaining one card among many
   *  where the surrounding context is part of the point.
   *
   *  `{ dim: true }` blurs and darkens everything except the target, and blocks
   *  clicks outside it. That is a much heavier hand and is reserved for the
   *  moments the tour genuinely needs ONE thing done before it can continue —
   *  currently just the daily-plan nudge, where every following beat assumes a
   *  plan exists. Using it for a beat the user could reasonably skip turns a
   *  walkthrough into a hostage situation. */
  spotlight(selector: string | null, opts?: { dim?: boolean }): void
  /** Hand control to the user and resolve when they click `selector`.
   *
   *  Resolves anyway after `fallbackMs` (default 7s). That fallback is not
   *  optional politeness — without it a user who does not understand the
   *  instruction, or whose target never rendered, is stuck in a modal
   *  walkthrough with no way forward, which is the single worst outcome
   *  available here. Returns whether the user actually clicked, so a beat can
   *  tell "they did it" from "we moved on for them".
   *
   *  Also resolves (as `true`) if the target LEAVES the DOM — a modal closed
   *  with Escape or a backdrop click is the step being completed, not a miss. */
  waitForClick(selector: string, fallbackMs?: number): Promise<boolean>
  /** Hand control to the user and resolve once the input/textarea at `selector`
   *  is non-empty — or, with `{ settled: true }`, once it has also stopped
   *  changing for a moment.
   *
   *  The typed-input counterpart to `waitForClick`, and the reason the planner
   *  beats can be guided at all: the composer's buttons are DISABLED until their
   *  field has content, and a disabled button never fires a click, so a
   *  click-only walkthrough can do nothing but sit there while the user types.
   *
   *  `settled` is for waiting on a field the APP fills (the AI draft landing in
   *  the title box), where the first character arriving is not the event worth
   *  reacting to. Same fallback contract as `waitForClick`: resolves `false`
   *  rather than stranding anyone. */
  waitForValue(selector: string, opts?: { fallbackMs?: number; settled?: boolean }): Promise<boolean>
  /** Resolve true if `selector` shows up within `timeoutMs`, false if it never
   *  does.
   *
   *  The primitive that lets a beat BRANCH ON WHAT THE APP DID rather than on
   *  what the script assumed. It exists for the draft beat: pressing "Draft with
   *  AI" with no provider connected produces a connect card instead of a draft,
   *  and a walkthrough that carried on narrating the draft would be describing a
   *  screen the user is not looking at. Distinct from `waitForElement`'s internal
   *  use inside `point`/`waitForClick`, which treat a miss as "degrade quietly";
   *  here the miss is the answer. */
  appeared(selector: string, timeoutMs?: number): Promise<boolean>
  /** Sleep, aborting early if the walkthrough is skipped. */
  pause(ms: number): Promise<void>
  /** Open one of the shell's REAL modals (or `null` to close).
   *
   *  Narrow by design: the only modals the walkthrough opens are ones it is
   *  genuinely handing the user — the planner and Settings. Anything the tour
   *  merely DEMONSTRATES is drawn by the tutorial's own surface instead, so it
   *  never renders the user's real data as an example (see `demoSummary`). */
  openModal(modal: StageModal): void
  /** Show or hide the walkthrough's own end-of-day summary card.
   *
   *  Not the real `DaySummaryOverlay`: that reads the user's actual day, which
   *  is empty on a fresh install and is their genuine work on any other — the
   *  one thing the example day exists to avoid putting words in front of. */
  demoSummary(on: boolean): void
  /** Open Settings at a specific section — how the walkthrough hands the user
   *  the REAL provider picker and tracker-connect UI rather than a tutorial-only
   *  imitation of them. Whatever they do there is a real, persisted change. */
  openSettings(section: SettingsSection): void
  /** Open a day-task card's detail panel, or clear the selection with `null`.
   *
   *  Selecting dispatches a real click on the card rather than constructing a
   *  `DayTaskDetail` directly: that type carries layout-derived fields
   *  (`hue`, `footLo`/`footHi`, laid-out segments) computed inside
   *  `DayTaskColumn`, so building one here would fork the layout maths and
   *  drift from it. Clicking exercises the same path the user's own click does.
   *
   *  Used for the fallback case — a beat whose `waitForClick` timed out still
   *  needs the panel open for the beats that follow. When the user did click,
   *  this is a harmless second selection of the card they already chose. */
  selectTask(id: string | null): void
  /** Ask the user a question and resolve with the `value` of the option they
   *  pick, or `null` if they skip the walkthrough while it is open.
   *
   *  This is the one primitive with no equivalent in the marketing demo, and it
   *  exists because the walkthrough now BRANCHES: someone working off a Jira or
   *  Linear board and someone keeping their own list need different next steps,
   *  and guessing wrong means either pushing a tracker connect at a solo user or
   *  hiding it from the person who most needs it. There is deliberately no
   *  timeout — unlike `waitForClick`, nothing sensible can be assumed on the
   *  user's behalf here, and the walkthrough is skippable at any moment anyway. */
  ask(question: string, options: StageChoice[]): Promise<string | null>
  /** Play the full-screen title sequence and resolve when it is done.
   *
   *  The one beat that advances on its own. It used to be two centred cards
   *  behind a "Let's go" button, which made the product ask permission to start
   *  something the user had just asked for, and put a click between them and a
   *  voiceover that begins the moment the tour does. Everything AFTER this is
   *  back on Next, where reading speed is the user's to set. */
  intro(lines: string[]): Promise<void>
  /** Narrate `text` and wait for the user to press Next.
   *
   *  This is the walkthrough's default way to move between beats, replacing the
   *  fixed `pause(ms)` it used to run on. Timed advance is wrong here for two
   *  independent reasons: a voiceover plays over these lines and cannot be
   *  synced to hardcoded millisecond guesses, and a reader who is slower than
   *  the guess simply loses the sentence. `pause` still exists, but only for
   *  mechanical gaps (letting a modal finish opening), never for reading time. */
  next(text: string, label?: string, opts?: { center?: boolean }): Promise<void>
  /** Swap the timeline (and the right panel) between the user's REAL day and
   *  the pre-built example day.
   *
   *  The walkthrough runs in two halves. The first teaches the two things
   *  Meridian needs FROM the user — a daily plan, and optionally a tracker — and
   *  has to run against their own empty timeline, because "this is blank right
   *  now and that is expected" is exactly the lesson. The second demonstrates
   *  what Meridian gives BACK, which needs a populated day that a fresh install
   *  cannot have. Flipping this is the seam between them. */
  showExample(on: boolean): void
  /** True once the user has skipped; every beat should bail on it. */
  readonly aborted: boolean
}

/** One answer to a [`Stage.ask`] question. `value` is what the beat branches on;
 *  `label` is what the user reads. */
export interface StageChoice {
  label: string
  value: string
  /** Optional second line under the label — for a choice whose consequence is
   *  not obvious from three words. */
  hint?: string
}

/** The subset of the shell's `ActiveModal` the walkthrough drives. Narrower than
 *  the shell's own union on purpose: the walkthrough only ever opens the two
 *  surfaces it is actually handing over to the user. */
export type StageModal = 'settings' | 'plan' | null

/** Raised inside a beat when the user skips, to unwind the script immediately
 *  rather than letting it run on invisibly against a torn-down overlay. */
export class Aborted extends Error {
  constructor() {
    super('walkthrough aborted')
    this.name = 'Aborted'
  }
}

/** Resolve after `ms`, or reject with [`Aborted`] the moment `signal` fires —
 *  so a skip mid-`pause` unwinds now instead of after the remaining delay. */
export function sleep(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal.aborted) return reject(new Aborted())
    const id = setTimeout(() => {
      signal.removeEventListener('abort', onAbort)
      resolve()
    }, ms)
    function onAbort() {
      clearTimeout(id)
      reject(new Aborted())
    }
    signal.addEventListener('abort', onAbort, { once: true })
  })
}

/** Centre point of `selector` in viewport coordinates, or `null` if it is not
 *  on screen. Used to fly the cursor and to place the spotlight ring. */
export function centreOf(selector: string): { x: number; y: number } | null {
  const el = document.querySelector(selector)
  if (!el) return null
  const r = el.getBoundingClientRect()
  if (r.width === 0 && r.height === 0) return null
  return { x: r.left + r.width / 2, y: r.top + r.height / 2 }
}

/** Wait until `selector` exists in the DOM, up to `timeoutMs`.
 *
 *  Beats routinely point at things that appear in response to the beat before
 *  (a modal's contents, a freshly rendered detail panel). Querying immediately
 *  races React's commit, so a beat would intermittently point at nothing. Polls
 *  on animation frames rather than a MutationObserver because the miss case
 *  needs a deadline more than it needs precision. */
export function waitForElement(
  selector: string,
  signal: AbortSignal,
  timeoutMs = 2000,
): Promise<Element | null> {
  return new Promise((resolve, reject) => {
    if (signal.aborted) return reject(new Aborted())
    const deadline = performance.now() + timeoutMs
    let raf = 0
    function onAbort() {
      cancelAnimationFrame(raf)
      reject(new Aborted())
    }
    signal.addEventListener('abort', onAbort, { once: true })
    const tick = () => {
      const el = document.querySelector(selector)
      if (el || performance.now() > deadline) {
        signal.removeEventListener('abort', onAbort)
        resolve(el)
        return
      }
      raf = requestAnimationFrame(tick)
    }
    tick()
  })
}
