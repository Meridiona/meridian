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
  /** TYPE `text` into the input/textarea at `selector`, a character at a time,
   *  as though the tour were at the keyboard. Resolves `false` if the field is
   *  not on screen - same contract as every other primitive that targets one.
   *
   *  For the beats where waiting on the user costs more than it teaches. The
   *  planner's first beat used to hand over a blank box and wait: a first-time
   *  user who has not yet seen the product do anything is asked to invent an
   *  example of a thing they have not been shown, in a field whose format they
   *  are guessing at, before the tour will move. Typing it instead spends four
   *  seconds and leaves them with a filled form and a button to press.
   *
   *  IT DRIVES THE REAL FIELD, not the store behind it. React owns `value`, so a
   *  plain assignment is discarded on the next render and no `onChange` fires -
   *  hence the prototype's native setter plus a bubbling `input` event, which is
   *  the pair React's synthetic layer actually reads. Going around it to the
   *  composer's own `setNote` would be shorter and would quietly stop being a
   *  demonstration: the same reasoning as `demoDrag`, which presses the product's
   *  real "+ Add" rather than reaching into the plan. */
  type(selector: string, text: string): Promise<boolean>
  /** Synchronous read of the input/textarea at `selector`'s current value, or
   *  `''` if it is not on screen.
   *
   *  A one-off peek for a beat that needs to BRANCH on what a field already
   *  holds rather than wait on it — e.g. checking whether a drafted title
   *  already clears a length floor before deciding whether to say anything
   *  about it. Not a substitute for `waitForValue`: this never waits. */
  value(selector: string): string
  /** Hand control to the user and resolve once the input/textarea at
   *  `selector` holds at least `minWords` whitespace-separated words —
   *  checked on arrival, then on every change, until it clears the floor or
   *  `fallbackMs` elapses. Resolves `false` on timeout, and also `false` if the
   *  field disappears (the composer closed) - same contract as
   *  `waitForClick`/`waitForValue`, which answer `false` for a target that is
   *  not there. Only a title that actually cleared the floor resolves `true`,
   *  because callers gate "the task can be saved" on this.
   *
   *  `waitForValue`'s `settled` only asks "is there SOMETHING here" — the
   *  composer's `canCreate` asks "is there ENOUGH here" (`MIN_TITLE_WORDS`),
   *  and the two can disagree: a drafted title is non-empty the moment it
   *  lands, but the model is not guaranteed to hit the word floor its own
   *  prompt asks for. Pointing at "Add to today" over a title that is short
   *  rings a button a click can never satisfy. This waits on the actual gate
   *  instead — polling rather than an input-event listener for the same
   *  reason `waitForValue` does: a programmatic edit (the model still
   *  revising) does not reliably fire the same event a keystroke does. */
  waitForMinWords(selector: string, minWords: number, opts?: { fallbackMs?: number }): Promise<boolean>
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
  /** PERFORM a drag from `from` onto `to`, with the tour's own cursor, and land
   *  the real state change at the end of it. Resolves false if either element is
   *  not on screen.
   *
   *  SHOWN, NOT ASKED. Dragging is the one gesture in the planner that is not a
   *  button, which is exactly why the tour used to hand it over ("drag the ones
   *  you are doing today") and wait. That failed two ways at once: a first-time
   *  user does not know a card is draggable until they have seen one move, and
   *  the beat then blocked on a gesture some of them would never attempt - in a
   *  modal, with the tour stalled behind it. A demonstration teaches the same
   *  thing in two seconds and cannot be failed. They still have their own board
   *  in front of them and every other card to drag afterwards.
   *
   *  The move at the end is REAL, not staged: the ghost lands and then the app's
   *  own add-to-today path runs, so the card the user just watched fly across is
   *  genuinely in their plan and genuinely saved. A demonstration that reverts
   *  the moment it finishes teaches the user that nothing here is real. */
  demoDrag(from: string, to: string): Promise<boolean>
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
  /** Raise the REAL end-of-day scheduling dialog (`WorklogAutoGenerateDialog`),
   *  and whatever the user picks is really saved.
   *
   *  The one ask left after the demonstration, and the only one that could not
   *  have been made earlier: it asks what time to wrap up a day, which means
   *  nothing until the user has seen what gets wrapped up. Ordinarily the shell
   *  raises this by itself on a first Timeline load; during the tour that would
   *  fire in the middle of a beat, so it is suppressed there and handed to the
   *  script to place. */
  showWorklogSchedule(on: boolean): void
  /** Open Settings at a specific section — how the walkthrough hands the user
   *  the REAL provider picker and tracker-connect UI rather than a tutorial-only
   *  imitation of them. Whatever they do there is a real, persisted change.
   *
   *  `lock` makes the step REQUIRED: the modal loses its exits until the step is
   *  actually done, and hands the flow back on its own once it is. Only defensible
   *  because the tour's own Skip control still sits above the modal, so the user is
   *  never truly trapped - and because every locked step carries a way to say the
   *  ask does not apply to them (see [`LockOutcome`]). */
  openSettings(section: SettingsSection, opts?: { lock?: boolean }): void
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
  next(text: string, label?: string, opts?: { center?: boolean; celebrate?: boolean }): Promise<void>
  /** Replay a whole working day, 9 AM to 6 PM, at about a second an hour.
   *
   *  THE BEAT THE PRODUCT IS HARDEST TO EXPLAIN WITHOUT. Everything up to here
   *  is something the user did: they wrote a plan, they connected a board. The
   *  timeline is the opposite - it is the half that happens while they are not
   *  looking, which is exactly why a static screenshot of a full day does not
   *  teach it. Shown finished, it reads as a report someone filled in. Shown
   *  filling, an hour at a time, off a clock they can watch, it reads as what it
   *  is: the product working on its own. Same pixels, completely different
   *  claim, and the claim is the whole pitch.
   *
   *  Runs against `showExample`'s day, revealing each card as the clock reaches
   *  the hour that card's work started in - so the timeline builds in the real
   *  component, with the real layout, rather than in a bespoke animation that
   *  would drift from it. Ported from the marketing demo's build phase
   *  (meridiona-website/assets/js/demo.js, `advanceNow` + the `stops` loop).
   *
   *  `marks` is the narration, keyed to wall-clock hours, so the copy stays in
   *  the script with every other line the tour says. */
  replayDay(marks: ReplayMark[]): Promise<void>
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

/** One narration line during [`Stage.replayDay`], pinned to the wall-clock hour
 *  it should appear on (9 = 9 AM … 18 = 6 PM). Keyed to the hour rather than to
 *  a millisecond offset so re-pacing the replay cannot silently desync the words
 *  from what is on screen. */
export interface ReplayMark {
  hour: number
  say: string
}

/** The card the walkthrough is currently flying across the screen, in viewport
 *  coordinates. `html` is a clone of the real card's markup - the board holds
 *  whatever the user's own tracker returned, so the tour cannot draw a
 *  stand-in that would look like their data without BEING their data.
 *
 *  `dx`/`dy` start at 0 and are set once, a beat later, so the CSS transition
 *  has two frames to interpolate between. */
export interface GhostCard {
  html: string
  left: number
  top: number
  width: number
  height: number
  dx: number
  dy: number
}

/** One answer to a [`Stage.ask`] question. `value` is what the beat branches on;
 *  `label` is what the user reads. */
export interface StageChoice {
  label: string
  value: string
  /** Optional second line under the label — for a choice whose consequence is
   *  not obvious from three words. */
  hint?: string
  /** Optional illustration above the label, named rather than passed as JSX so
   *  the script file stays free of React and readable as pure narrative.
   *
   *  Worth the extra concept for exactly one kind of question: the branch. The
   *  team-or-solo ask is the first real decision the tour puts to someone who has
   *  been using the product for ninety seconds, it decides the shape of
   *  everything after it, and as two blocks of prose it was a reading
   *  comprehension test - "Jira, Linear, GitHub Issues" is a list a user has to
   *  parse before they can recognise themselves in it. Their tracker's actual
   *  logo is recognised before the sentence is read. */
  art?: 'trackers' | 'solo'
}

/** The subset of the shell's `ActiveModal` the walkthrough drives. Narrower than
 *  the shell's own union on purpose: the walkthrough only ever opens the two
 *  surfaces it is actually handing over to the user. */
export type StageModal = 'settings' | 'plan' | null

// ── Is the walkthrough on right now? ─────────────────────────────────────────
//
// A module flag, not a context or a prop. The one reader is `PlanView`, which is
// three components below the shell that owns the tour (shell → PlanModal →
// PlanView) and has nothing else to do with it - threading a `tutorial` prop
// through both would put a walkthrough concern in two components that are
// otherwise entirely unaware of it, and every future reader would widen the
// same chain. Same shape and the same reasoning as `useTaskComposer`'s resume
// flag and the `meridian:connect-ai` window event.
//
// Read at decision time (inside a callback), never during render: this is
// deliberately not reactive, and a component that re-rendered off it would be
// relying on a value nothing publishes changes for.
let running = false

/** Set by [`useTutorial`] when the script starts and when it stops. */
export const setTutorialRunning = (on: boolean) => { running = on }

/** True while the walkthrough is driving the UI.
 *
 *  Exists so a real screen can behave differently when it is being DEMONSTRATED
 *  rather than used. The first case is the plan's suggestion pre-fill: it seeds
 *  Today with what looks active, which is right on an ordinary morning and wrong
 *  in a tour, where the very next beat asks the user to drag their work across.
 *  A demonstration whose action has already been performed for them teaches
 *  nothing - they watch a thing they were about to be taught. */
export const isTutorialRunning = () => running

/** How a locked Settings step ended. `'declined'` is the user saying the ask does
 *  not apply to them - "I don't use a project tool" - which is a completion, not a
 *  failure, and the tour says something different for each. */
export type LockOutcome = 'connected' | 'declined'

let lockOutcome: LockOutcome | null = null

/** Recorded by `SettingsModal` as it releases a locked step.
 *
 *  The tour cannot read this from the DOM: by the time it is asked, the modal that
 *  would have shown the answer has closed. And it cannot come back as a return
 *  value, because the tour does not CALL the modal - it opens one and waits for the
 *  screen behind it to reappear. */
export const setLockOutcome = (o: LockOutcome) => { lockOutcome = o }

/** Read and clear. One-shot, like the composer's resume flag: a stale outcome read
 *  by a later beat would narrate the wrong branch of a question already answered. */
export const consumeLockOutcome = (): LockOutcome | null => {
  const was = lockOutcome
  lockOutcome = null
  return was
}

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

/** Animate `from` → `to` over `ms`, calling `onUpd` every frame, aborting with
 *  [`Aborted`] if the walkthrough is skipped mid-flight.
 *
 *  The counterpart to [`sleep`] for the one beat that animates a VALUE rather
 *  than waiting out a duration — the day replay's clock and reveal line. Eased
 *  (cubic in-out) so each hour arrives and settles rather than lurching, which
 *  is what makes nine of them in a row read as a day passing. */
export function tween(
  from: number, to: number, ms: number, signal: AbortSignal,
  onUpd: (value: number) => void,
): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal.aborted) return reject(new Aborted())
    const t0 = performance.now()
    let raf = 0
    function onAbort() {
      cancelAnimationFrame(raf)
      reject(new Aborted())
    }
    signal.addEventListener('abort', onAbort, { once: true })
    const frame = (now: number) => {
      const p = Math.min(1, (now - t0) / ms)
      const e = p < 0.5 ? 4 * p * p * p : 1 - Math.pow(-2 * p + 2, 3) / 2
      onUpd(from + (to - from) * e)
      if (p < 1) { raf = requestAnimationFrame(frame); return }
      signal.removeEventListener('abort', onAbort)
      resolve()
    }
    raf = requestAnimationFrame(frame)
  })
}

/** The attribute marking the walkthrough's own surface. Set on `TutorialScreen`'s
 *  root; read by [`tourTarget`]. */
export const TOUR_SURFACE_ATTR = 'data-tour-surface'

/** Resolve a tour selector, PREFERRING the walkthrough's own surface.
 *
 *  THE BUG THIS EXISTS FOR. The walkthrough draws an opaque replica over the live
 *  dashboard - it does not replace it - so the real shell is still mounted
 *  underneath with its real toolbar, and both carry the same `data-tour` hooks
 *  (that is the point: the replica teaches where the real control lives). The
 *  shell renders BEFORE `{tutorial.screen}`, so a bare `document.querySelector`
 *  returns the hidden real one every time. The ring and the cursor then land on
 *  its coordinates - near the replica, but not on it, because the real toolbar
 *  carries day arrows and a date the replica does not - and the beat can only
 *  ever finish on its fallback timer, since the element the user can actually
 *  press is the one that was not measured.
 *
 *  Falls back to the whole document, because plenty of anchors are deliberately
 *  outside the surface: the real settings modal, the worklog schedule dialog,
 *  every modal the tour opens for real. */
export function tourTarget(selector: string): Element | null {
  const surface = document.querySelector(`[${TOUR_SURFACE_ATTR}]`)
  return surface?.querySelector(selector) ?? document.querySelector(selector)
}

/** Centre point of `selector` in viewport coordinates, or `null` if it is not
 *  on screen. Used to fly the cursor and to place the spotlight ring. */
export function centreOf(selector: string): { x: number; y: number } | null {
  const el = tourTarget(selector)
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
      const el = tourTarget(selector)
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
