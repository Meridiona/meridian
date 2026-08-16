//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

// The first-run walkthrough, beat by beat.
//
// TWO HALVES, and the seam matters. Part one runs on the user's OWN timeline,
// which on a fresh install is empty — that emptiness is the lesson, not a
// problem to hide, and it is where the two things Meridian needs FROM the user
// are collected (a daily plan; optionally a tracker). Part two flips to a
// pre-built example day (`Stage.showExample`) to show what it gives BACK, which
// no fresh install can demonstrate with real data. Demonstrating on the fake
// day FIRST would make the real one, seen a minute later, read as broken.
//
// Ordering principle, and the reason the tracker and AI-provider asks are here
// rather than in the setup wizard: EVERY CONFIGURATION ASK LANDS IMMEDIATELY
// AFTER THE MOMENT THAT MAKES IT OBVIOUSLY WORTH DOING. Asked upfront, "connect
// your Jira" reads as a data grab and "pick an AI model" means nothing, because
// the user has not yet seen a worklog draft or anything a model wrote. The
// tracker ask sits right after they enter a daily plan, where "pull your real
// tickets in instead of typing them" is the obvious next click; the model ask
// sits right after they read two things a model wrote. Same asks, very
// different conversion. Moving either back into the wizard undoes the point.
//
// The tracker ask is also a BRANCH, not a prompt: a solo user and a board user
// need different products from here on, and `Stage.ask` is the one primitive
// with no equivalent in the marketing demo for exactly that reason.
//
// Copy is carried from the public marketing demo where it already existed
// (meridiona-website/assets/js/demo.js `runSummaryDemo`), so the product says
// what the website promised, in words already reviewed for public display.
//
// # Who calls this
// [`useTutorial`] runs this against the `Stage` it builds.
//
// # Related
// - `./engine.ts` — the `Stage` contract every beat is written against
// - `./scriptDay.ts` — part two, which this hands off to at the seam

import { availableTrackerNames } from '@/lib/integrations'
import { MIN_TITLE_WORDS, titleWords } from '@/components/plan/useTaskComposer'
import { consumeLockOutcome, type Stage, type TakeoverBeat } from './engine'
import { runDayHalf } from './scriptDay'

/** Human "2m 14s" / "47s" from whole seconds.
 *
 *  No longer used by a beat (see the opening of `runScript`) — it formats
 *  `setup_elapsed_secs` for the setup card, which is the surface that duration
 *  actually belongs on. */
export function fmtSetupElapsed(secs: number): string {
  if (secs < 60) return `${secs}s`
  const m = Math.floor(secs / 60)
  const s = secs % 60
  return s ? `${m}m ${s}s` : `${m}m`
}

/** The two story beats the tour opens on, before any product is on screen.
 *
 *  Copy lives here with every other line the walkthrough says, rather than in
 *  the component that animates it — the tour's whole script should be readable
 *  in one file, and the wording is reviewed far more often than the animation.
 *
 *  OPENS ON THE PROBLEM, NOT ON ITSELF. The old title sequence said a tour was
 *  starting and that it was quick - two true facts about the tour, told to
 *  someone with no reason yet to care about it. These name the day they just
 *  had and what it cost them, and hand over at the point where "there is another
 *  way" is worth hearing. What Meridian actually DOES is still the first
 *  narration beat's job, said over their own screen where it means something.
 *
 *  THE SECOND SUB IS LOAD-BEARING, not scene-setting: a tour that takes the
 *  whole window has to say it ends. It survives verbatim from the old opening,
 *  and `tutorial-sample-day` pins it.
 *
 *  NO MINUTE COUNT, in that line or anywhere else. It once said "about 2
 *  minutes", which was true of the old flow and stopped being true when the tour
 *  took over connecting a tracker and an AI CLI: an OAuth round-trip through a
 *  browser, a board to pick, an install that may or may not already be there.
 *  Most of that time is spent outside our window, on someone else's site, so no
 *  number we print can be right for both the user who has Jira open in the next
 *  tab and the one chasing an admin for approval. A promise a first-run screen
 *  visibly breaks costs more trust than the number ever bought - which is also
 *  why the progress row shows dots and never a denominator (see `CHAPTERS`). */
export const OPENING: TakeoverBeat[] = [
  {
    kicker: 'Meridian',
    // Illustrative, and written to be recognisable rather than accurate - the
    // tour cannot know what they did yesterday. The shape of the day is the
    // point, and the second line is what makes it land.
    line: "Yesterday you fixed a bug, reviewed a teammate's change, and sat in two meetings.",
    sub: "By tonight you'll remember about half of it.",
    // The mark and wordmark ride this beat only. Everywhere else they would be
    // branding for its own sake, in front of someone already using the product.
    mark: true,
    // No button. See `Stage.takeover`.
    hold: 2600,
  },
  {
    kicker: 'The part nobody likes',
    line: 'Then you spend the last twenty minutes writing it all down.',
    sub: 'Digging back through commits, tabs and Slack to reconstruct a day you already lived. '
      + "We'll take you around - it's quick.",
    cta: "There's another way",
  },
]

/** The seam: everything before it was the user giving Meridian something, and
 *  everything after is Meridian showing what it does back.
 *
 *  The script already treated this as its turning point - it is where the
 *  confetti used to be, and was removed from for marking the wrong thing (the
 *  finish line is the last beat of `scriptDay`, not this). A takeover is what
 *  that moment actually wanted: a pause, a change of surface, and one line
 *  saying the asking is over.
 *
 *  It does NOT stand in front of the replay. `scriptDay` argues, correctly, that
 *  the nine-hours-in-twelve-seconds build narrates itself and that anything
 *  placed before it is a step between the user and the one thing that half
 *  exists to show. This sits on the near side of the seam - after the planner
 *  closes, before the example day begins. */
export const SEAM: TakeoverBeat = {
  kicker: 'Six hours later',
  line: 'You just worked.',
  sub: 'Here is what Meridian wrote down while you did.',
  cta: 'Show me the day',
}

/** A card in the board column. The tour cannot name a ticket - the board is
 *  whatever the user's own tracker returned - so every beat that needs one
 *  addresses it structurally. */
const BOARD_CARD = '[data-tour="plan-board"] [data-plan-card]'

/** The first task the tour writes into the planner on the user's behalf.
 *
 *  DELIBERATELY THE THING THEY ARE DOING. This lands in their REAL plan - part
 *  one runs against their own day, not the example - so it cannot be a fiction
 *  like "fix the flaky login test" without leaving a made-up task on a real
 *  board. Setting Meridian up is what this person is genuinely spending the
 *  next few minutes on, which makes it the one sentence that is both a good
 *  demonstration and true.
 *
 *  Written the way the field asks to be written - lowercase, unpunctuated, the
 *  shape of something said rather than filed - because the beat after it is the
 *  model turning exactly that into a titled task. A tidy sentence here would
 *  make the draft look like formatting.
 *
 *  AND IT IS LOAD-BEARING ON `MIN_TITLE_WORDS`. Beat 2c-2 exists because a
 *  drafted title can land under the four-word floor `canCreate` enforces, and
 *  the example recorded there is an onboarding note ("Testing onboarding" →
 *  "Test onboarding") - the same subject as this one. While every user wrote
 *  their own note that was a per-user lottery; with one sentence feeding every
 *  first run, whatever the model does with THIS input it does for everyone. A
 *  terse note would put the whole fleet on the "this title's a little thin"
 *  branch, which asks for exactly the typing this beat removed, at a worse
 *  moment.
 *
 *  So it carries two verbs and two objects on purpose - enough for a faithful
 *  title to clear the floor without padding. Shortening it is not a copy edit;
 *  check the drafted title still clears four words first. The branch degrades
 *  gracefully either way (it names what is missing and waits on the real gate),
 *  so this is a quality floor, not a correctness one. */
const FIRST_TASK_NOTE = 'setting up meridian and going through onboarding'

/**
 * Wait for the user's tickets to land in the planner, narrating the wait.
 *
 * NOT a probe. `available` is read from the local DB, and a tracker connected
 * thirty seconds ago has not populated it yet - `PlanView` fires one catch-up
 * `sync_tasks` when it opens onto an empty board, and that call goes out to Jira
 * or Linear over the network. The first version of this beat gave it 2.5s, which
 * is roughly a tenth of what a first Jira sync takes, so the tour concluded there
 * were no tickets and walked a user who had just imported a board through writing
 * a task by hand - while their tickets arrived behind the modal it had moved on
 * to. Falling through is the right behaviour for a genuinely empty project and
 * the wrong behaviour for a slow one, and only the clock tells them apart.
 *
 * The line is there because 25 seconds of a still screen after a connect reads as
 * the connect having hung. Cleared either way, so nothing stale survives into the
 * next beat.
 *
 * MOSTLY A BACKSTOP NOW. The sync no longer starts when the planner opens: the
 * connect flow warm-starts it the moment a tracker becomes usable (`@/lib/taskSync`)
 * and the Settings hand-back holds itself open until it lands, so the wait is spent
 * on a screen that has just said something true and finished rather than in front of
 * an empty board. By the time this runs the tickets are usually already there and it
 * returns immediately. It stays because the hand-back's hold is capped, and because
 * a board can be empty for reasons that never went near a connect.
 *
 * CHECKED BEFORE NARRATING, not after. `appeared(sel, 0)` reads the DOM once with no
 * wait, so the common case - the hand-back already held the screen until the board
 * had cards - sees them and returns immediately without saying anything. Saying "the
 * first sync takes a moment" AFTER the screen has already moved to a board full of
 * tickets contradicts what the user is looking at: the plan they were just handed
 * back to reads as already done, and a line claiming otherwise reads as the tour
 * having lost track of what just happened. The line is reserved for the genuine
 * backstop case, where the board is still empty when this beat runs.
 */
async function waitForTickets(s: Stage): Promise<boolean> {
  if (await s.appeared(BOARD_CARD, 0)) return true
  s.say('Pulling your tickets in - the first sync takes a moment.')
  const got = await s.appeared(BOARD_CARD, 25000)
  s.say('')
  return got
}

/**
 * DEV ONLY: skip part one and open straight into the example day.
 *
 * Part two is the half that gets iterated on - the replay, the worklog flow, the
 * summary - and reaching it honestly costs a plan, a tracker connect (an OAuth
 * round-trip through a browser) and a provider install. Sitting through that on
 * every edit is how a walkthrough stops being edited.
 *
 * Triggered by `?tour=day` (the shell already treats any `tour` param as "run
 * the walkthrough", so the one flag both starts it and places it) or by setting
 * `localStorage['meridian.walkthrough.dev-from'] = 'day'`, which survives the
 * reload that a `?tour=day` URL forces on you.
 *
 * Hard-gated on a non-production build. It bypasses two REQUIRED steps - the
 * plan and the AI-provider connect - so an installed user who landed on this
 * would finish the tour with a Meridian that cannot write anything.
 */
function devDayHalfOnly(): boolean {
  if (process.env.NODE_ENV === 'production') return false
  if (typeof window === 'undefined') return false
  try {
    if (new URLSearchParams(window.location.search).get('tour') === 'day') return true
    return window.localStorage.getItem('meridian.walkthrough.dev-from') === 'day'
  } catch {
    return false
  }
}

/**
 * Run the walkthrough. Resolves when the last beat finishes; throws `Aborted`
 * (from `engine.ts`) if the user skips, which the caller treats as a normal
 * outcome rather than an error.
 */
export async function runScript(s: Stage): Promise<void> {
  // Dev shortcut: straight into the example-day half. The context it is handed is
  // the "everything already set up" one, because the point is to land on the
  // replay in one step, not to exercise a branch.
  if (devDayHalfOnly()) {
    await runDayHalf(s, { ai: 'already-working', usesTracker: 'tracker' })
    return
  }

  // WHAT WE KNOW ABOUT THE USER'S AI, and how we learned it. Beat 6 reads this to
  // decide whether to run the picker at all, and which sentence is true.
  //
  // This used to be one boolean set only by the no-provider detour, which got the
  // ALREADY-CONNECTED user badly wrong: their draft just works, the connect card
  // never appears, the flag stays false, and beat 6 tells someone with a working
  // provider "Last thing: pick which one" and opens a picker they do not need.
  // A successful draft IS evidence - it cannot happen without a provider - so it
  // counts here too.
  //
  // `null` means no evidence either way: they never typed, so nothing ever asked
  // the model for anything, and beat 6 must still run.
  let ai: 'connected-in-tour' | 'already-working' | null = null

  // Answered in beat 1b, read again in beat 7 — which is why it is declared out
  // here rather than inside the planner block that sets it. The closing worklog
  // line promises a post to the user's board, and promising that to someone who
  // told us they have no board would be a lie the product contradicts within the
  // hour. Null means the question was never reached (they never opened the
  // planner), which the `!== 'tracker'` branch handles correctly by default.
  let usesTracker: string | null = null

  // ══ PART ONE — on the user's OWN, empty day ═══════════════════════════════
  // Everything up to `showExample(true)` runs against their real timeline,
  // which on a fresh install is blank. That is deliberate: this half is about
  // what Meridian needs FROM them, and the blank screen is the honest starting
  // condition they are about to leave. Demonstrating on a fake populated day
  // first would make the real one, seen thirty seconds later, read as broken.

  // ── 1. The opening, as a story rather than a title card ─────────────────
  // TWO STORY BEATS BEFORE ANY PRODUCT. This used to be a title sequence - the
  // mark, the name, and two lines saying a tour was starting. That framed the
  // tour but sold nothing: it told a first-time user what was about to happen to
  // them, at the one moment they have no reason yet to care.
  //
  // These say what the day they just had cost them, and hand over at the point
  // the answer is worth hearing. Everything after is the same tour it always
  // was; what changed is that it now opens on the problem instead of on itself.
  //
  // They stand in front of nothing - there is no product on screen yet - which
  // is why the first advances on a `hold` while the second waits on its button.
  // A button on the very first card makes the product ask permission to start
  // something the user just asked for. From the second beat onward, advance is
  // under their thumb, where reading speed is theirs to set.
  s.chapter('intro')
  for (const beat of OPENING) await s.takeover(beat)

  // ── 2. The daily plan — the first thing it needs ────────────────────────
  // Straight from the title card into a hand on the wheel. There are no
  // explain-the-product beats between the two: a tour that opens with a
  // paragraph about what Meridian is has spent the user's attention before
  // asking for anything, and the paragraph means more AFTER they have put their
  // own tasks in than before. What the product does is shown, in the beats that
  // follow, on their own plan.
  //
  // The one beat that dims. Everything but the nudge blurs out and stops taking
  // clicks, because this is the single step the rest of the tour is built on:
  // beat 3 asks where those tasks come from, and the closing beats describe
  // matching work against them. A user who wanders off here gets a tour about a
  // plan they never made. Everywhere else the spotlight is a ring only — see
  // `Stage.spotlight` — and Skip is still one click away in the corner.
  // The tracker connect lives inside this chapter rather than being one of its
  // own - a user who says "solo" would otherwise watch the progress row lose a
  // step. See `CHAPTERS`.
  s.chapter('plan')
  s.say("First: tell Meridian what you're working on today. Go ahead and click here.")
  s.spotlight('[data-tour="plan-open"]', { dim: true })
  await s.point('[data-tour="plan-open"]')
  // Long, because it is now the ONLY thing on screen to do: the fallback exists
  // so a target that never rendered cannot strand anyone, not to time the user
  // out of a decision.
  const openedPlan = await s.waitForClick('[data-tour="plan-open"]', 120000)
  s.spotlight(null)
  if (openedPlan) {
    // ── 1b. Team or solo — asked HERE, before the composer ─────────────────
    //
    // This question used to sit at the very end of the tour, after the plan and
    // the example day. It moved to the front because the answer decides what the
    // NEXT five minutes look like, and asking it last meant every beat before it
    // was written for a user we had not identified yet: someone on a Jira board
    // was walked through hand-writing a task while their real tickets sat
    // unimported two clicks away.
    //
    // It is also the moment the ask is cheapest. The planner is open and visibly
    // empty; "where should these come from?" is a question the user is already
    // asking. The same words upfront, in a setup wizard, read as a data grab -
    // which is exactly why the wizard no longer has an Integrations step (see
    // `app/setup/steps.tsx`, where INTEGRATIONS_STEP sits unused and says so).
    // The click above deliberately opened NOTHING - see `useTutorial`'s no-op
    // `onOpenPlan`. The question comes first, on a clear screen: a task board
    // behind the dialog is the biggest thing on it and pulls at a user who has
    // just been handed a decision, and it is not evidence for either answer
    // anyway, being empty. What fills it is precisely what is being asked. The
    // planner opens below, either straight after a solo answer or when the
    // connect flow hands back.
    // ILLUSTRATED, and the question shortened to match. It used to name the
    // trackers in the question itself, which made the one real decision in the
    // tour a list a user has to parse before they can recognise themselves in it.
    // The logos say the same thing faster and without reading: someone who lives
    // in Jira knows which card is theirs before they reach the label. See
    // `StageChoice.art`.
    //
    // The names come from `availableTrackerNames()`, never a written-out list.
    // Promising a tracker that is flagged `comingSoon` (Linear, today) on the one
    // screen where the user is choosing whether they HAVE one is how someone
    // picks this branch for a tool they then cannot connect.
    usesTracker = await s.ask(
      'Where do your tasks come from?',
      [
        { label: 'A team board', value: 'tracker', hint: `${availableTrackerNames()} - pull my real tickets in`, art: 'trackers' },
        { label: 'My own list', value: 'solo', hint: 'I keep track of my own work', art: 'solo' },
      ],
    )

    if (usesTracker === 'tracker') {
      // Straight in. There used to be a "Let's connect it" beat with a Connect it
      // button here, which made the user confirm the thing they had just chosen -
      // "I'm on a team board" IS the consent, and asking again reads as the tour
      // not having heard them. The sentence survives as narration over the modal
      // opening, where it explains rather than gates.
      s.say('Meridian pulls your open tickets straight in, so your plan comes from your real board.')
      // Locked. Same contract as the AI step: no exit until the step is answered
      // one way or the other, and SettingsModal hands the flow back by itself.
      // Defensible only because the tour's Skip still floats above the modal, and
      // because "I don't use a project tool" is on screen from the first frame -
      // see IntegrationsSection's gate block for why that is not a footnote.
      s.openSettings('integrations', { lock: true })
      // Long enough to READ the line above, not just the modal's open transition -
      // it is now the only place the "why" is said, since the Connect it beat that
      // used to hold it is gone. 1400 is the ceiling: the pause budget in
      // tutorial-sample-day.test.ts caps mechanical gaps at 1500ms.
      await s.pause(1400)
      s.say('Pick your tool and sign in. If yours is not here, say so at the bottom - nothing later depends on it.')

      // WAIT FOR THE RELEASE, NOT FOR THE PLANNER. The lock resolves the moment
      // the user connects or declines; the planner returns a second and a half
      // later. Waiting on the planner meant that in between - the exact seconds
      // after someone pressed "I don't use a project tool" - the tour was still
      // telling them to pick a tool and sign in. Instruction contradicting the
      // decision they just made, while the app was busy agreeing with it.
      //
      // Very generous: a real OAuth round-trip through a browser, and on Jira and
      // GitHub a project picker after it.
      await s.appeared('[data-tour="lock-handback"]', 600000)
      // Which branch they took. Read ONCE - the flag is consumed - so it goes into
      // a local that the beats below can read as many times as they need.
      const declined = consumeLockOutcome() === 'declined'
      // AND THE ANSWER CHANGES. `usesTracker` was set to 'tracker' by the question,
      // but the question asked what they use, not what they connected - and beat 7
      // reads this to decide whether to promise "Meridian posts these to your
      // board". Left as 'tracker', someone who just told us they have no project
      // tool would be promised a post to it, which the product contradicts within
      // the hour. What they have NOW is a personal list, so say so.
      if (declined) usesTracker = 'solo'
      // Clear the stale instruction immediately. Not replaced with a new sentence
      // here: the modal's own confirmation is on screen saying the right thing,
      // and a second line over the top of it is two voices at once.
      s.say('')
      // EITHER selector, because the planner has two layouts and which one comes
      // back depends on the answer just given. With a tracker connected the board
      // has tickets, so it renders the two columns and `plan-today` exists. With
      // none it renders the hero composer instead (`PlanView`'s `boardEmpty`) and
      // `plan-today` never mounts - so waiting on it alone left a declined user
      // staring at nothing for the full ten-minute fallback.
      await s.appeared('[data-tour="plan-today"], [data-tour="task-note"]', 600000)
      await s.pause(700)

      if (declined) {
        // THE FULL EXPLANATION, on a card that takes the screen. This is the one
        // moment in the tour where the product just lost the feature the user was
        // being sold, and "let's write this one by hand instead" made that sound
        // like the consolation prize it is not: writing a task here IS the flow,
        // AI does the shaping, and the tracker only ever added import and posting.
        // Said small and in passing, a user reasonably concludes they picked the
        // broken path and starts looking for how to undo it.
        await s.next(
          "No problem at all. You can create tasks right here in seconds - write a line, and Meridian's AI turns it into a proper task on your board. Connect a tool later and it will sync both ways.",
          "Show me",
          { center: true },
        )
      } else if (await waitForTickets(s)) {
        // 1c. The drag - ONLY if a ticket actually came in. Connecting a tracker
        // does not guarantee any: a brand-new Jira project, a filter that matches
        // nothing, or a sync that has not finished all leave the board empty, and
        // `PlanView` then renders its hero composer with no board column at all.
        // Told to "drag the ones you are doing today" in front of that, the user
        // looks for tickets that do not exist and concludes the connect failed.
        // Falling through to the composer beats is both true and useful, so the
        // guard costs nothing when it fires. Anchored on a CARD rather than on the
        // Today column, because the column renders whether or not anything landed
        // in the other one - which is exactly the case this is guarding against.
        //
        // Named as the payoff of the thing they just did. The connect finished a
        // few seconds ago in a different modal, and the tickets appearing here is
        // the only visible evidence it worked - saying so out loud closes that
        // loop, and this is the last chance to.
        await s.next('Your tickets came straight in - they are on the right. Anything you are working on today goes in the left column.', 'Show me')

        // SHOWN, NOT ASKED. This used to be "drag the ones you are doing today"
        // followed by a Done button, and it was the worst beat in the tour: a
        // first-time user does not know a card can be picked up until they have
        // seen one move, so the tour was blocking - inside a modal - on the one
        // gesture it had given them no reason to believe in. Some never tried it
        // and pressed Done on an empty plan. Watching it happen teaches the same
        // thing in two seconds, cannot be failed, and leaves them a whole board of
        // cards to try it on themselves. The move is real and saved - see
        // `Stage.demoDrag`.
        s.say('')
        await s.demoDrag(BOARD_CARD, '[data-tour="plan-today"]')
        await s.pause(600)
        s.spotlight('[data-tour="plan-today"]')
        await s.next('Like that. Drag across as many as you want - every change saves itself.', 'Got it')
        s.spotlight(null)

        // 1d. The bridge to the composer. Someone who just imported a board does
        // not assume they can also WRITE here - they assume tasks come from Jira
        // and this is a viewer. Saying otherwise is worth one beat, because it is
        // the difference between Meridian being a mirror and being where they work.
        //
        // Said as WHY, not as a capability. The old line was "You can also write
        // a task here, without opening your board at all", which named a
        // permission the user had no reason to want yet and quietly raised the
        // wrong question - does this create a ticket, or not? The real case is
        // the work that never had a ticket, which every day has, and the answer
        // to where it goes comes with it.
        s.say('Not everything you do has a ticket. Add that work here too - Meridian tracks it the same way.')
        s.spotlight('[data-tour="plan-new-task"]')
        await s.point('[data-tour="plan-new-task"]')
        // THIS one they press themselves. The drag above is a demonstration
        // because the gesture is unfamiliar; a button is not, and the composer
        // beats that follow are about to ask them to type - so the hand has to be
        // on the wheel by the time they get there.
        await s.waitForClick('[data-tour="plan-new-task"]', 120000)
        s.spotlight(null)
        await s.pause(600)
      }
    }
    // NOW the planner opens - the only place it does on the solo path, since the
    // nudge the user clicked is a no-op during the tour. Idempotent on the tracker
    // branch, where SettingsModal's hand-back already opened it, so both answers
    // land on the same screen for the composer beats below.
    s.openModal('plan')
    await s.pause(500)
    // The solo branch says nothing and adds no beat. "Nothing here needs a
    // tracker" is a reassurance about a worry the user has not voiced, and it
    // spends a beat defending a choice they just made confidently. The composer
    // below is the answer to their answer.
    // They are now in the REAL planner, and the next four beats walk one task
    // all the way in. This used to be a single "add a few tasks" line and a wait
    // on the close button, which left a first-time user alone in front of a form
    // with a note box, an AI button and two more fields, having to guess which
    // to touch first — the exact moment a tour is for.
    //
    // No dim in here. The modal already isolates the form, and dimming would
    // fence off its own close button; the ring is enough when there is nothing
    // else on screen competing.
    // No framing beat before this. A Next-gated paragraph on top of a form the
    // user is looking at is a paragraph in the way — it asks for a click that
    // teaches nothing and delays the one that does. Every beat in here points at
    // exactly one thing to type or press, and says only what that thing is for.
    //
    // 2a. The note. THE TOUR TYPES IT, and this is a reversal worth recording.
    //
    // It used to ring the empty box and wait (`waitForValue`, up to three
    // minutes) for the user to write their own first task. The reasoning was
    // that a plan made of their real work is worth more than a demo of one, and
    // that is true - but it asked for the wrong thing at the wrong moment.
    // Nothing has happened yet: the user has not seen Meridian draft anything,
    // has no idea what "however you'd say it out loud" buys them over a title,
    // and is being asked to invent an example in a field whose format they are
    // guessing at, before the tour will move at all. The commonest outcomes are
    // a placeholder-shaped non-answer and a three-minute stall.
    //
    // So the tour writes one, and it writes the task the user is ACTUALLY doing:
    // they are setting Meridian up, right now, and that is a real first task
    // rather than a fiction. The beats that follow are unchanged and the user
    // still presses every button - what changes is that they now watch the
    // product turn one scruffy line into a titled task, which is the thing this
    // stretch exists to show and which no amount of waiting was going to teach.
    //
    // The field is theirs afterwards: it is a textarea holding text, and every
    // later beat edits rather than dictates.
    s.say('Your first task goes here - one line, however you would say it out loud.')
    s.spotlight('[data-tour="task-note"]')
    await s.point('[data-tour="task-note"]')
    const typed = await s.type('[data-tour="task-note"]', FIRST_TASK_NOTE)
    // A moment to read it back before the next ring moves. Typing that lands and
    // is immediately covered by the next instruction reads as a flicker.
    if (typed) await s.pause(900)

    if (typed) {
      // 2b. The AI draft — the first time they see the model do anything.
      //
      // …unless the connect card is ALREADY on screen, which it is whenever the
      // composer was left holding it: the notice is component state that
      // survives the modal being closed and reopened, and the tour reaches this
      // line again on a replay. Ringing "Draft with AI" in front of a card that
      // says "Meridian needs an AI engine" points the user at the one button
      // that cannot do the thing being described - it will only re-raise the
      // same card. The ring belongs on whatever is actually asking for a click.
      //
      // Zero-ish timeout: this is a "is it there right now" probe, not a wait.
      const askingAlready = await s.appeared('[data-tour="ai-connect"]', 250)

      if (!askingAlready) {
        s.say('Now let Meridian shape it into a task - your own AI, on your machine.')
        s.spotlight('[data-tour="task-draft"]')
        await s.point('[data-tour="task-draft"]')
        await s.waitForClick('[data-tour="task-draft"]', 60000)
      }

      // 2b-i. …except on a machine with no AI connected, which is MOST first
      // runs. The press then produces a connect card instead of a draft, and a
      // tour that carried on with "a title and a description, from one line"
      // would be narrating a screen the user is not looking at — the single
      // most confusing thing a walkthrough can do.
      //
      // Timed generously: the composer holds a "Checking your AI…" beat and the
      // card fades in over ~600ms, both deliberate (see AiEngineNotice). This
      // waits out that whole arrival rather than racing it.
      if (askingAlready || await s.appeared('[data-tour="ai-connect"]', 4000)) {
        ai = 'connected-in-tour'
        // Let it finish arriving and be read before anything else moves. The
        // user pressed a button and got an unexpected answer; a spotlight
        // landing on top of that in the same second is a second surprise.
        await s.pause(1200)
        // "Drafting needs", not "Meridian does that" - "that" pointed back at a
        // draft they had just watched fail, and reads as nothing at all when the
        // card was already up and no draft was ever attempted.
        s.say('Drafting needs an AI engine of your own - you pick which. Connect one and we will come straight back.')
        s.spotlight('[data-tour="ai-connect"]')
        await s.point('[data-tour="ai-connect"]')
        await s.waitForClick('[data-tour="ai-connect"]', 120000)
        s.spotlight(null)
        // The planner is gone: the connect event swaps the shell's one modal
        // slot for Settings. The half-written note survives it — the composer's
        // state is a module store precisely so this kind of detour cannot eat it.
        //
        // Settings opens LOCKED here, which is also what puts the picker on its
        // subscription question rather than the provider grid (SettingsModal
        // passes `gate={!!lock}`). So the first thing to narrate is that
        // question, not "pick a provider" — which would be describing the screen
        // after next.
        await s.pause(1000)
        s.say('First, the honest question: do you already pay for one of these?')
        // No spotlight. Both answers are real and the tour must not appear to
        // push the paid one — the recommendation is carried by the badge on the
        // card, which is as far as it should go. Ringing one card would make the
        // other read as the wrong answer.
        await s.waitForClick('[data-tour="gate-subscription"], [data-tour="gate-free"]', 180000)
        await s.pause(900)
        // One line covering both branches, because which one they are on is now
        // obvious from what is in front of them, and a branch here would need to
        // re-read the DOM to find out.
        //
        // It does NOT say "close this when you are done": the modal is on a
        // required step, so its corner button is disabled until a provider is
        // actually connected. Telling someone to close a thing that will not
        // close is worse than saying nothing.
        s.say('Follow it through - Meridian sets up the rest.')
        // NOT a wait on a close button. Once the provider is written, the app
        // takes itself back to the planner and starts the draft the user had
        // already asked for (SettingsModal's `onLockSatisfied` → `armResume`),
        // so the thing to wait for is the planner being BACK. Asking them to
        // close a modal that closes itself would either race the app or leave
        // the tour narrating a screen that had already moved on.
        //
        // Long, because everything slow lives in here: installing a CLI, a
        // browser sign-in, or getting a Groq key from a website.
        await s.appeared('[data-tour="task-note"]', 600000)
        await s.pause(900)
        // The draft is already running by now - `armResume` starts it. So this
        // hands over to the same beats the normal path uses rather than asking
        // for a click that has effectively already happened.
        s.say('Connected. Meridian is drafting your task now.')
        await s.pause(1400)
      } else {
        // No connect card means the draft ran, which means a provider is already
        // set up and working. Recorded so beat 6 congratulates them instead of
        // marching them into a picker for something they have already done.
        ai = 'already-working'
      }

      // 2c. The draft landing. `settled` because the fields fill progressively —
      // reacting to the first character would put the next line on screen
      // mid-write, while the thing it describes is still appearing.
      //
      // NARRATED AFTER, NOT DURING, and it no longer invites an edit. The line
      // used to be "A title and a description, from one line. Edit anything you
      // like." on screen while the model was still writing - so the tour was
      // asking the user to correct a sentence that had not finished appearing,
      // about work the model had in fact got right. "Edit anything you like" is a
      // true statement about the form and the wrong thing to say the second after
      // the AI has done the job it was just sold on: it plants a doubt, and it
      // stalls a user who now feels they are supposed to find something to fix.
      // The fields are plainly editable - they are two text boxes with a cursor in
      // them - and nothing later depends on the user knowing it here.
      s.spotlight(null)
      await s.waitForValue('[data-tour="task-title"]', { fallbackMs: 90000, settled: true })
      // AND the description, because Add to today needs BOTH - `canCreate` in
      // useTaskComposer.ts requires a title of MIN_TITLE_WORDS *and* a
      // non-empty description. Waiting on the title alone rang a button that
      // was still disabled: the model fills the fields in order, so there is a
      // real window where the title has landed and the description has not, and
      // "Happy with it? Add it to today" over a greyed-out button reads as the
      // app having stalled. Same fallback, so a model that never writes a
      // description still lets the user through to type one themselves.
      await s.waitForValue('[data-tour="task-desc"]', { fallbackMs: 90000, settled: true })
      await s.pause(500)
      s.spotlight(null)

      // 2c-2. EDGE CASE: a drafted title can still land short. `canCreate` in
      // useTaskComposer.ts requires MIN_TITLE_WORDS regardless of whether the
      // title got there by hand or by the model, and the model - however well
      // its prompt is worded - is not guaranteed to hit that floor on a terse
      // note ("Testing onboarding" drafted down to the two-word "Test
      // onboarding" is what surfaced this). The beat below used to skip
      // straight to "That is the task. Add it to today." and point at a
      // button that stays disabled for the rest of its 90s fallback, then
      // said "Saved." regardless of whether anything was. Checked explicitly
      // instead: name what's missing, and wait on the real gate rather than
      // assume the draft cleared it.
      let titleReady = titleWords(s.value('[data-tour="task-title"]')) >= MIN_TITLE_WORDS
      if (!titleReady) {
        s.spotlight('[data-tour="task-title"]')
        s.say("This title's a little thin - add a few more words about the work before you can save it.")
        await s.point('[data-tour="task-title"]')
        titleReady = await s.waitForMinWords('[data-tour="task-title"]', MIN_TITLE_WORDS, { fallbackMs: 120000 })
        s.spotlight(null)
      }

      if (titleReady) {
        // 2d. WHERE IT GOES - the one decision on this form with a consequence
        // outside Meridian, and the only one the tour stops for.
        //
        // Renders only with a tracker connected (TaskComposer's `boardProvider`), so
        // it is probed rather than assumed - a solo user has no choice to make and
        // gets no beat. Left undiscovered, this is how someone files a real ticket
        // onto a shared board by accident, or writes three days of personal notes
        // wondering why their team never sees them. Both are cheap to prevent here
        // and expensive to explain afterwards.
        //
        // No spotlight on either option and no recommendation: which one is right
        // depends entirely on what they just wrote, and the tour has no view on it.
        if (await s.appeared('[data-tour="task-where"]', 1200)) {
          s.say('One choice: keep it to yourself, or file it as a real ticket your team can see. Pick either - you can change it per task.')
          s.spotlight('[data-tour="task-where"]')
          await s.point('[data-tour="task-where"]')
          await s.waitForClick('[data-tour="task-where"] [role="radio"]', 120000)
          s.spotlight(null)
          await s.pause(600)
        }

        // 2e. Commit it.
        s.say('That is the task. Add it to today.')
        s.spotlight('[data-tour="task-add"]')
        await s.point('[data-tour="task-add"]')
        await s.waitForClick('[data-tour="task-add"]', 90000)
        s.spotlight(null)
        await s.pause(700)
        s.say('Saved. Add as many as you like - each one saves itself - then close this when you are done.')
      } else {
        // They never lengthened it. Same honesty as the "never typed" branch
        // below: say what's true and let them close it, rather than claim a
        // save that never happened.
        s.say('No rush - finish it whenever you like. Close this when you are ready.')
      }
    } else {
      // They never typed. Say what the form was for and let them close it;
      // pressing on with "now click Draft" would point at a disabled button.
      s.spotlight(null)
      s.say('You can add tasks here whenever you like - type a note and Meridian drafts the rest. Close this when you are ready.')
    }
    // RING THE CLOSE BUTTON. Both lines above end by telling the user to close the
    // modal, and then pointed at nothing - leaving them to hunt the one control the
    // tour is actually blocked on, in the corner, at the end of a beat where every
    // other instruction came with a ring. The cursor goes there too: this is the
    // last thing asked of them in the planner, and it should be as findable as the
    // first.
    s.spotlight('[data-tour="modal-close"]')
    await s.point('[data-tour="modal-close"]')
    await s.waitForClick('[data-tour="modal-close"]', 180000)
    s.spotlight(null)
    s.openModal(null)
    await s.pause(600)
    // ── 2f. The seam ────────────────────────────────────────────────────────
    // Everything before this beat was the user giving Meridian something, and
    // everything after it is Meridian showing what it does back.
    //
    // NO CONFETTI HERE ANY MORE. This used to be the tour's one celebration, on
    // the reasoning that the seam deserved marking. It does, but it is not the
    // finish line - the last beat of `scriptDay.ts` is - and a tour that throws
    // confetti twice has thrown it at nothing. The flourish moved there; the
    // milestone is still marked, by the sentence and the full-screen card.
    //
    // AND IT SAYS THE PLAN IS OPTIONAL. The line here used to be "that is the
    // only thing Meridian ever needs from you", which is flatly untrue in the
    // direction that matters: Meridian tracks the day whether or not anyone
    // plans it - the timeline the very next beat demonstrates is built from
    // capture, not from this list. Told the plan is required, a user who skips
    // it one morning reasonably assumes the product stopped working, and a user
    // who does not want a planner at all concludes the product is not for them.
    // What the plan actually buys is sharper matching, so say that instead.
    await s.next(
      "Nice - today's tasks are in. This part is optional, by the way: Meridian tracks your day either way. A plan just helps it match your work to the right thing.",
      'What happens now?',
      { center: true },
    )
  } else {
    // The nudge never rendered or they ignored it — say what it was for and
    // move on rather than stalling on a control they are not going to press.
    await s.next("You can set that up any time from the panel on the right - Meridian works better when it knows what you are aiming at.")
  }

  // ── 3. (was: solo or on a board) ────────────────────────────────────────
  // GONE, deliberately. This used to be where the tour asked "team board or your
  // own list?" and, on `tracker`, opened Settings → Integrations unlocked.
  //
  // It moved to beat 1b, inside the planner, and grew teeth: the answer now
  // decides the shape of everything after it, so asking it here meant every
  // earlier beat was written for a user we had not identified. Asking twice
  // would be worse than either position - the second ask reads as the app
  // having forgotten, on the one screen whose job is to look attentive.
  //
  // Nothing replaces it. Both branches are fully resolved by the time the tour
  // reaches this point.

  // ══ PART TWO — the timeline, BUILT rather than shown ══════════════════════
  // Everything from here lives in `./scriptDay.ts`. The split is the tour's own
  // seam, not an arbitrary line drawn to shorten a file: part one collects what
  // Meridian needs FROM the user and runs on their real, empty timeline; part two
  // demonstrates what it gives BACK, on a pre-built example day. Both halves grew
  // past the point where one file could be read top to bottom, which is the whole
  // reason the script is written as narrative in the first place.
  //
  // The seam gets a beat of its own, on the near side: the planner has closed,
  // the example day has not begun, and there is nothing on screen worth pointing
  // at. See `SEAM`.
  await s.takeover(SEAM)
  await runDayHalf(s, { ai, usesTracker })
}
