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
// - `./sampleDay.ts` — `FOCUS_TASK_ID` / `DRAFT_TASK_ID`, the cards beats target

import type { Stage } from './engine'
import { DRAFT_TASK_ID, FOCUS_TASK_ID } from './sampleDay'

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

/** The two lines under the wordmark in the title sequence — an opening and its
 *  footnote, revealed a word at a time.
 *
 *  Copy lives here with every other line the walkthrough says, rather than in
 *  the component that animates it — the tour's whole script should be readable
 *  in one file, and the wording is reviewed far more often than the animation.
 *
 *  Two facts, no more: that this is a tour, and how long it takes. What
 *  Meridian actually does is the first narration beat's job, said over the
 *  user's own screen where it means something; stacking it here turned the open
 *  into a four-card slideshow in front of someone who has not seen the product
 *  yet. */
export const INTRO_LINES = [
  "Let's get started.",
  "We'll take you around - it takes about 2 minutes.",
]

/** Selector for a day-task card, matching the `data-task-id` DayTaskColumn
 *  stamps on each `TaskBand`. Centralised so a markup change breaks one line
 *  rather than every beat. */
const card = (id: string) => `[data-task-id="${id}"]`

/**
 * Run the walkthrough. Resolves when the last beat finishes; throws `Aborted`
 * (from `engine.ts`) if the user skips, which the caller treats as a normal
 * outcome rather than an error.
 *
 * `hasRealTasks` is whether the user's OWN day already has folded tasks. Only
 * the opening beat reads it, and only to avoid telling someone with a populated
 * timeline that their timeline is empty.
 */
export async function runScript(s: Stage, hasRealTasks = false): Promise<void> {
  // Set by the draft beat's no-provider detour. Beat 6 reads it and drops its own
  // provider ask, so nobody is walked through the picker twice in one tour.
  let connectedAi = false

  // ══ PART ONE — on the user's OWN, empty day ═══════════════════════════════
  // Everything up to `showExample(true)` runs against their real timeline,
  // which on a fresh install is blank. That is deliberate: this half is about
  // what Meridian needs FROM them, and the blank screen is the honest starting
  // condition they are about to leave. Demonstrating on a fake populated day
  // first would make the real one, seen thirty seconds later, read as broken.

  // ── 1. Title sequence ───────────────────────────────────────────────────
  // Named as a tour up front. An earlier version dropped the user mid-gesture
  // ("click this card") with no frame for what was happening or how long it
  // would take, which reads as the app seizing control rather than offering
  // something.
  //
  // This is the ONLY beat that advances on a timer, and it is the only one that
  // should: there is nothing on screen to point at yet, so there is nothing for
  // a slower reader to lose. From beat 2 on, every narration line waits on Next,
  // because a voiceover plays over them and cannot be synced to hardcoded
  // millisecond guesses. `pause` survives only for mechanical gaps (a modal
  // finishing its open transition).
  await s.intro(INTRO_LINES)

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
  s.say("First: tell Meridian what you're working on today. Go ahead and click here.")
  s.spotlight('[data-tour="plan-open"]', { dim: true })
  await s.point('[data-tour="plan-open"]')
  // Long, because it is now the ONLY thing on screen to do: the fallback exists
  // so a target that never rendered cannot strand anyone, not to time the user
  // out of a decision.
  const openedPlan = await s.waitForClick('[data-tour="plan-open"]', 120000)
  s.spotlight(null)
  if (openedPlan) {
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
    // 2a. The note. `waitForValue`, not `waitForClick`: nothing here is
    // clickable until they have typed something, so a click wait would sit
    // silently through the one part of the tour they are meant to do.
    s.say("Write your first task here - however you'd say it out loud.")
    s.spotlight('[data-tour="task-note"]')
    await s.point('[data-tour="task-note"]')
    const typed = await s.waitForValue('[data-tour="task-note"]', { fallbackMs: 180000 })

    if (typed) {
      // 2b. The AI draft — the first time they see the model do anything.
      s.say('Now let Meridian shape it into a task - your own AI, on your machine.')
      s.spotlight('[data-tour="task-draft"]')
      await s.point('[data-tour="task-draft"]')
      await s.waitForClick('[data-tour="task-draft"]', 60000)

      // 2b-i. …except on a machine with no AI connected, which is MOST first
      // runs. The press then produces a connect card instead of a draft, and a
      // tour that carried on with "a title and a description, from one line"
      // would be narrating a screen the user is not looking at — the single
      // most confusing thing a walkthrough can do.
      //
      // Timed generously: the composer holds a "Checking your AI…" beat and the
      // card fades in over ~600ms, both deliberate (see AiEngineNotice). This
      // waits out that whole arrival rather than racing it.
      if (await s.appeared('[data-tour="ai-connect"]', 4000)) {
        connectedAi = true
        // Let it finish arriving and be read before anything else moves. The
        // user pressed a button and got an unexpected answer; a spotlight
        // landing on top of that in the same second is a second surprise.
        await s.pause(1200)
        s.say('Meridian does that with an AI engine of your own - you pick which. Connect one and we will come straight back.')
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
      }

      // 2c. The draft landing. `settled` because the fields fill progressively —
      // reacting to the first character would put the next line on screen
      // mid-write, while the thing it describes is still appearing.
      s.spotlight('[data-tour="task-title"]')
      s.say('A title and a description, from one line. Edit anything you like.')
      await s.waitForValue('[data-tour="task-title"]', { fallbackMs: 90000, settled: true })
      await s.pause(500)

      // 2d. Commit it.
      s.say('Happy with it? Add it to today.')
      s.spotlight('[data-tour="task-add"]')
      await s.point('[data-tour="task-add"]')
      await s.waitForClick('[data-tour="task-add"]', 90000)
      s.spotlight(null)
      await s.pause(700)
      s.say('That is one. Add as many as you like - then close this when you are done.')
    } else {
      // They never typed. Say what the form was for and let them close it;
      // pressing on with "now click Draft" would point at a disabled button.
      s.spotlight(null)
      s.say('You can add tasks here whenever you like - type a note and Meridian drafts the rest. Close this when you are ready.')
    }
    s.spotlight(null)
    await s.waitForClick('[data-tour="modal-close"]', 180000)
    s.openModal(null)
    await s.pause(600)
    await s.next("That's your plan for the day - you can change it whenever you like.")
    // The empty timeline, named only NOW. It used to be explained before the
    // plan beat, which meant the first thing the tour did was apologise for a
    // blank screen. Here it answers a question the user is actually asking
    // ("I entered my tasks - why is the left side still empty?"), and it is the
    // natural place to say the fold is hourly. Anyone REPLAYING on a working
    // day sees a full timeline, and telling them it is empty would be the first
    // thing the tour got visibly wrong — so the line adapts.
    await s.next(hasRealTasks
      ? 'On the left is your day so far. Meridian builds that itself, an hour at a time, from what you actually work on.'
      : "Your timeline on the left is still empty - that's expected. Meridian fills it in as you work, an hour at a time.")
  } else {
    // The nudge never rendered or they ignored it — say what it was for and
    // move on rather than stalling on a control they are not going to press.
    await s.next("You can set that up any time from the panel on the right - Meridian works better when it knows what you are aiming at.")
  }

  // ── 3. Solo or on a board — the branch ──────────────────────────────────
  // This used to be a late, unconditional "connect your tracker" beat. It moved
  // here and became a question for one reason: the two answers need genuinely
  // different products. Someone on a Jira board wants their real tickets pulled
  // in so drafts can post back to them; someone keeping their own list needs to
  // hear, early and plainly, that nothing here depends on a tracker. Asking
  // costs one click and removes the chance of pitching the wrong one.
  const usesTracker = await s.ask(
    'Do you track your work on a team board - Jira, Linear, GitHub Issues - or keep your own list?',
    [
      { label: 'I use a team board', value: 'tracker', hint: 'Pull my real tickets in' },
      { label: 'I keep my own list', value: 'solo', hint: 'Just my own tasks' },
    ],
  )

  if (usesTracker === 'tracker') {
    await s.next("Let's connect it. Meridian pulls your open tickets in, so your plan comes from your real board - and it can post your updates back there later.", 'Connect it')
    s.openSettings('integrations')
    await s.pause(1000)
    s.say('Pick your tracker and sign in. Close this when you are done - you can always add or change it later in Settings.')
    // Generous, because this is a real OAuth round-trip through a browser.
    await s.waitForClick('[data-tour="modal-close"]', 300000)
    s.openModal(null)
    await s.pause(600)
  } else if (usesTracker === 'solo') {
    await s.next("Perfect - nothing here needs a tracker. Your timeline, your daily tasks and your summaries all work exactly the same. You can connect one later from Settings if that ever changes.")
  }

  // ══ PART TWO — on a pre-built example day ═════════════════════════════════
  // Now that the two asks are done, show what the payoff actually looks like.
  // This needs a full day of folded work, which no fresh install can have, so
  // the timeline swaps to the example — labelled as one for its whole duration
  // by the badge the overlay renders.
  s.showExample(true)
  await s.pause(700)
  await s.next("Now here's what your day will look like once Meridian has been running. This is an example, not your data.")

  // ── 4. Multi-sitting folding — the first real "how did it know that" ────
  // FOCUS_TASK_ID is the two-segment card. Handing the click to the user in the
  // very first beat sets the tone that this is something they operate.
  s.say('This one was worked in two sittings across the morning. Click it.')
  s.spotlight(card(FOCUS_TASK_ID))
  await s.point(card(FOCUS_TASK_ID))
  await s.waitForClick(card(FOCUS_TASK_ID), 6000)
  s.spotlight(null)
  s.selectTask(FOCUS_TASK_ID)
  await s.pause(600)
  await s.next('Meridian pieced it together from what was actually on screen - and wrote up what got done. That write-up is on the right.')

  // ── 5. The daily summary ────────────────────────────────────────────────
  s.selectTask(null)
  await s.next('At the end of each day, all of it rolls up into a summary you can actually hand to someone.', 'Show me')
  await s.point('[data-tour="summary-pill"]')
  s.click()
  s.demoSummary(true)
  await s.pause(900)
  await s.next('It says what you finished, what slipped, and what pulled you off plan - in language you can paste into a standup.')
  s.demoSummary(false)
  await s.pause(500)

  // ── 6. AI ask — motivated by what they just read ────────────────────────
  // They have now read two things a model wrote. THAT is what makes this ask
  // land, and why it cannot be moved earlier. The privacy framing is the same
  // one the marketing demo closes on, because it is the actual objection:
  // the model runs on the user's own CLI and subscription.
  //
  // SKIPPED ENTIRELY if the draft beat already took them through it. Sending
  // someone back to a provider screen they finished four beats ago reads as the
  // tour having lost track of them - and worse, as the connection they just made
  // not having counted. They still get the privacy line, which is the part of
  // this beat that was never about the picker.
  if (connectedAi) {
    await s.next('That writing is done by an AI model running on your own machine, through the provider you just connected - your work never sits on our servers.')
  } else {
    await s.next('That writing is done by an AI model running on your own machine, through your own CLI - your work never sits on our servers. Last thing: pick which one.', 'Pick a model')
    s.openSettings('intelligence')
    await s.pause(1000)
    s.say('Choose a provider - Meridian installs and signs you in right here. Close this when you are done, and you can change it any time.')
    await s.waitForClick('[data-tour="modal-close"]', 300000)
    s.openModal(null)
    await s.pause(500)
  }

  // ── 7. The worklog draft — the payoff ───────────────────────────────────
  // The money feature. The closing line splits on the answer given back in beat
  // 3: for a board user this is the thing they connected their tracker FOR, and
  // saying so closes that loop. For a solo user the same card is still useful
  // (the write-up is real), but promising them a post to a tracker they do not
  // have would be a lie the product contradicts an hour later.
  s.say('For work that maps to a ticket, Meridian drafts the update for you. Click this one.')
  s.spotlight(card(DRAFT_TASK_ID))
  await s.point(card(DRAFT_TASK_ID))
  await s.waitForClick(card(DRAFT_TASK_ID), 6000)
  s.spotlight(null)
  s.selectTask(DRAFT_TASK_ID)
  await s.pause(700)
  await s.next(usesTracker === 'tracker'
    ? 'Meridian posts these to your board for you - no ticket-hunting at the end of the day. Nothing goes out without your say-so.'
    : 'You get the write-up either way. Connect a tracker later and Meridian will post these for you too.')

  // ── 8. Handoff ──────────────────────────────────────────────────────────
  // The dangerous moment: the example day disappears and the real one is
  // empty except for the setup card. Naming that plainly is the whole job
  // here — an unexplained empty screen after a polished demo reads as the
  // product being broken. The promise made is deliberately event-shaped
  // ("as you work") rather than clock-shaped: the hourly fold only runs for
  // hours clearing its activity gate, so no specific time can be promised
  // without risking a broken one.
  s.selectTask(null)
  s.showExample(false)
  await s.pause(400)
  await s.next(
    "That is the whole product. Meridian is running in your menu bar from now on, and your own timeline fills in as you work. You can replay this any time from Settings.",
    'Finish', { center: true })
  s.say('')
}
