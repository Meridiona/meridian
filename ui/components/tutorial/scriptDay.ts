//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

// PART TWO of the first-run walkthrough: what Meridian gives BACK.
//
// Split out of `script.ts` when the closing half grew past the point where one
// file could hold both. The seam is the natural one the tour already had: part
// one runs on the user's OWN, empty timeline and collects the two things
// Meridian needs FROM them (a daily plan; optionally a tracker); part two flips
// to a pre-built example day (`Stage.showExample`) and demonstrates the output,
// which no fresh install can show with real data.
//
// The shape of this half is a single arc, and the order is load-bearing:
//
//   the day BUILDS      (replayDay)      - it happens on its own, hour by hour
//   one task, opened    (detail panel)   - and it is right about what you did
//   a worklog posted    (the real panel) - matched to a ticket you planned, 90%
//   the summary         (section beats)  - all of it rolls up
//   the OFF-PLAN work   (propose flow)   - and what didn't match gets a ticket written
//   the end-of-day time (real dialog)    - the one setting, asked once it means something
//   the off switch      (tray replica)   - and it is yours to stop
//
// The worklog appears TWICE on purpose, and they are not the same beat. The
// first is the easy case - planned work, matched to the ticket it was planned
// as - run inside the shipped detail panel so the user presses the real Generate
// and the real Approve. The second is the case that decides whether the product
// gets trusted: work that matched nothing, for which Meridian writes a brand-new
// ticket rather than forcing it onto the nearest half-match. Collapsing them
// into one demo would have to pick an outcome, and either choice teaches the
// user that the other one does not exist.
//
// Every ask in here lands after the thing that makes it obviously worth doing -
// the same rule part one is built on. "What time does your day end?" is a
// meaningless question until the user has watched a day get written up, and a
// tour that asked it in a setup wizard would get a shrug and a default.
//
// # Who calls this
// [`runScript`] in `./script.ts`, once part one has resolved both branches.
//
// # Related
// - `./TutorialSummaryCard.tsx` — the summary the section beats name, part by part
// - `./TutorialTray.tsx` — the menu-bar replica the pause beats drive
// - `./sampleDay.ts` — `FOCUS_TASK_ID`, the card the fold beat targets

import { load } from '@/lib/bridge'
import { connectedTrackers } from '@/lib/integrations'
import type { IntegrationsResponse } from '@/lib/api-types'
import type { Stage } from './engine'
import { FOCUS_TASK_ID, OFFPLAN_TASK_ID } from './sampleDay'
import { trayIsAtTheTop } from './TutorialTray'

/** Selector for a day-task card, matching the `data-task-id` DayTaskColumn
 *  stamps on each `TaskBand`. */
const card = (id: string) => `[data-task-id="${id}"]`

/** What part one learned, and part two has to stay consistent with. */
export interface DayHalfContext {
  /** What we know about their AI provider, and how we learned it. `null` means
   *  no evidence either way, which is the only case that still runs the picker. */
  ai: 'connected-in-tour' | 'already-working' | null
  /** `'tracker'` only if they both said they use a board AND did not decline the
   *  connect. A FALLBACK now, not the gate - see [`hasBoard`], which asks the
   *  integrations directly. It is still what answers when that read fails, since
   *  promising "this goes to your board" to someone who has none is a lie the
   *  product contradicts within the hour. */
  usesTracker: string | null
}

/** Is a tracker connected RIGHT NOW - asked of the integrations themselves.
 *
 *  The worklog stretch used to gate on `ctx.usesTracker`, a flag carried down
 *  from a question asked in part one and then rewritten by the connect flow's
 *  outcome. Three links, and any one of them breaking sends a user who HAS a
 *  board to the one-line "connect one some day" consolation instead of the beat
 *  that drives Generate, Approve and Posted - the single stretch the whole tour
 *  exists to land. It fails silently and looks like the tour simply skipping its
 *  best part, which is exactly how it was found.
 *
 *  So the branch asks the thing it actually depends on. `usesTracker` stays as
 *  the fallback for the one case the read cannot answer - it errored - where the
 *  answer the user gave is better than assuming either way. */
async function hasBoard(usesTracker: string | null): Promise<boolean> {
  try {
    const res = await load<IntegrationsResponse>('/api/integrations', 'get_integrations')
    return connectedTrackers(res).length > 0
  } catch {
    return usesTracker === 'tracker'
  }
}

export async function runDayHalf(s: Stage, ctx: DayHalfContext): Promise<void> {
  const { ai, usesTracker } = ctx
  const board = await hasBoard(usesTracker)

  // ── 3. The timeline, BUILT rather than shown ────────────────────────────
  // The half of the product that happens while the user is not looking, which is
  // precisely why it cannot be taught with a finished screenshot. A full day
  // presented whole reads as a report someone filled in; the same day arriving an
  // hour at a time, off a clock, reads as the app doing it - and that is the
  // entire claim Meridian makes. Same pixels, opposite meaning.
  //
  // NO PREAMBLE. There was a card here first - "on the left is your day so far,
  // here is a full day sped up" - behind a "Show me" button. It described what
  // was about to happen instead of letting it happen, in front of a timeline that
  // was deliberately blank at that moment, so it read as an explanation of
  // nothing. The replay is twelve seconds long and narrates itself as it goes;
  // anything that stands in front of it is a step between the user and the one
  // thing this half exists to show. It just plays.
  s.showExample(true)
  // NINE HOURS IN ABOUT TWELVE SECONDS. The marks are the narration, keyed to the
  // hour they belong on (see `Stage.replayDay`), and they are deliberately sparse:
  // this beat is watched, not read, and a line on every hour turns a thing
  // happening into a thing being described.
  //
  // "This is an example, not your data" is said FIRST, on the hour the first card
  // lands - the moment someone might otherwise mistake it for their own.
  //
  // THE NARRATION IS ABOUT THE HOUR RAIL, because that is now where the clock
  // is. The replay used to run a dial in the right-hand panel while the timeline
  // filled silently beside it, which put the passage of time in a decoration and
  // left the product surface looking like it was being filled from offstage. The
  // rail lights up hour by hour behind a descending now-line instead (see
  // `DayTaskColumn`'s `replayNowMin`), so "watch the times on the left" is a real
  // instruction with something to watch.
  await s.replayDay([
    // SHORT LINES. These are read at a glance, by someone whose eyes are on a
    // column filling itself - anything longer is prose nobody finishes, and the
    // next hour has arrived before they are halfway through it.
    { hour: 9, say: 'An example day, not your data.' },
    { hour: 11, say: 'Every hour, Meridian turns what you just did into a task. No timers, no notes.' },
    { hour: 14, say: 'It keeps going while you work, and tells you when something is ready.' },
    { hour: 16, say: 'Work you came back to twice becomes one task, not two.' },
    { hour: 18, say: 'Six o\'clock. The whole day, written up - and you logged none of it.' },
  ])

  // ── 3b. The finished day, taken in whole ────────────────────────────────
  // A BEAT TO LOOK AT IT. The replay used to end and hand straight over to
  // "open the big one in the afternoon" - a specific instruction about one card,
  // arriving while the thing the user actually just watched get built was still
  // being taken in. Nine hours of work resolve into a screen here, and that
  // screen is the product's whole answer to "where did my day go"; skipping past
  // it to a click spends the payoff on an errand.
  //
  // It rings the WHOLE view - timeline AND the glance panel on the right - because
  // the claim is about the day as one finished thing, and the right-hand
  // breakdown (hours, apps, categories) is half of what makes it an answer. A
  // ring this large falls back to the top narration bar; see `ringFits` in
  // TutorialOverlay.
  s.spotlight('[data-tour="day-view"]')
  await s.next('A whole day, written up while you worked - and you typed none of it.')
  s.spotlight(null)

  // ── 4. One task, opened — the first real "how did it know that" ─────────
  // FOCUS_TASK_ID is the card everything from here to the post happens on: two
  // segments (so the folding claim is visible) AND a ticket that is on the day's
  // plan (so the match, four beats later, is a strong and honest one). Handing
  // the click to the user in the very first beat sets the tone that this is
  // something they operate rather than watch.
  s.say('That is the day. Open the big one in the afternoon - it was worked in two sittings.')
  s.spotlight(card(FOCUS_TASK_ID))
  await s.point(card(FOCUS_TASK_ID))
  const opened = await s.waitForClick(card(FOCUS_TASK_ID), 60000)
  s.spotlight(null)
  // ONLY IF THEY DID NOT. `selectTask` opens the panel by synthesising a click on
  // the card, and the card's own handler TOGGLES - so calling it after a real
  // click shut the panel again the instant it opened. Everything after this ran
  // against a closed panel: the two ringed beats had nothing to ring, so their
  // captions fell back to the bar at the top of the window and explained a
  // "WHEN you did it" block that was not on screen.
  if (!opened) s.selectTask(FOCUS_TASK_ID)
  await s.pause(700)
  // And confirm it. The next four beats are ENTIRELY about what is in that panel,
  // so "is it actually open" is worth checking rather than assuming - a toggle is
  // one stray click away from being wrong, and the failure is silent: the
  // narration carries on describing a panel nobody can see. One retry, because if
  // the block is missing the panel is closed, which is exactly what a click fixes.
  if (!(await s.appeared('[data-tour="detail-when"]', 1500))) {
    s.selectTask(FOCUS_TASK_ID)
    await s.pause(700)
  }

  // TWO BEATS ON THE PANEL, one per block, each ringed. This was a single line
  // ending "that write-up is on the right" - which pointed at a panel with two
  // distinct claims in it and named neither. The panel is where the product
  // answers "how would it possibly know that?", and it answers it twice: WHEN
  // is the evidence (real clock times, and the break between them, reconstructed
  // from what was on screen), WHAT WAS DONE is the output. Read past as one
  // block, the times look like a label and the bullets look like something the
  // user might have typed. Ringed separately, they are two different proofs.
  s.spotlight('[data-tour="detail-when"]')
  await s.next('When you did it, worked out from your screen.')
  s.spotlight('[data-tour="detail-done"]')
  await s.next('What you did, written from the work itself.')
  s.spotlight(null)

  // ── 5. The worklog, end to end, on the card they just opened ────────────
  // THE FEATURE THE PRODUCT IS BOUGHT FOR, and the tour drives it rather than
  // describing it: Generate, a match with a number on it, Approve, Posted - four
  // states, each behind a control the user presses themselves. Narrated it reads
  // as a promise; operated it reads as a product, and the difference matters most
  // here, because "it writes my ticket updates for me" is the single claim a new
  // user is most entitled to disbelieve.
  //
  // The panel, its buttons and every transition are the SHIPPED ones. Only the
  // state behind them is scripted (`demoWorklog.ts`), because the example day's
  // task ids exist in no database and its ticket keys belong to another project -
  // the real generate could only error and the real approve could only file a
  // wrong comment on a stranger's board. The beat after the post says so.
  //
  // GATED ON THE TRACKER ANSWER, and not for tone. With no board connected the
  // panel deliberately shows "Connect a tracker to auto-log this" INSTEAD of
  // Generate (see `WorklogFooter`'s `noTracker` branch) - it will not offer a
  // draft with nowhere to go. So on the solo path there is no button to point at,
  // and a beat that waited for one would sit on a dead spotlight until it timed
  // out. They get the promise in a sentence, honestly tensed as something that
  // starts working when they connect something.
  if (board) {
    // TWO CLICKS, because the draft is its own surface now. The panel carries a
    // one-line status and a way in; the document, its matched tickets and its
    // controls live in a dialog (see WorklogDraftDialog for why they left the
    // panel). So the tour opens it, then presses Generate inside it.
    s.say('Now the part that saves the most time. Ask it to write the ticket update.')
    s.spotlight('[data-tour="wl-open"]')
    await s.point('[data-tour="wl-open"]')
    await s.waitForClick('[data-tour="wl-open"]', 120000)
    s.spotlight(null)
    await s.appeared('[data-tour="wl-generate"]', 4000)
    await s.pause(400)
    s.spotlight('[data-tour="wl-generate"]')
    await s.point('[data-tour="wl-generate"]')
    await s.waitForClick('[data-tour="wl-generate"]', 120000)
    s.spotlight(null)
    s.say('Reading the work, comparing it against the tasks you planned this morning, writing the update.')
    await s.appeared('[data-tour="draft-targets"]', 12000)
    await s.pause(600)

    // 5a. THE MATCH, WITH ITS NUMBER SHOWING. The confidence is the honest part:
    // it says the model made a judgement rather than looked something up, which
    // is what makes the override in the next line make sense.
    s.spotlight('[data-tour="draft-targets"]')
    await s.next('It matched this to a ticket on today\'s plan - 90%')
    s.spotlight(null)
    // NO BEAT ON THE RE-MATCH CONTROL. It used to ring it here and explain that
    // the call is always yours - a good sentence about the wrong moment. The
    // match on screen is a strong one the tour has just called out at 90%, so
    // "matched the wrong one?" answers a doubt nobody watching this has, and it
    // puts a detour in front of the only thing this stretch is here to land:
    // approve, and it posts. The override is one visible control away for anyone
    // who ever needs it, and beat 7 teaches picking a target properly - on the
    // off-plan task, where the choice is real.

    // 5b. Approve, then confirm. TWO presses, deliberately: this is the one place
    // work leaves Meridian and lands somewhere other people can see it.
    s.say('Nothing leaves Meridian until you approve it. Send this one.')
    s.spotlight('[data-tour="wl-approve"]')
    await s.point('[data-tour="wl-approve"]')
    await s.waitForClick('[data-tour="wl-approve"]', 120000)
    s.spotlight(null)
    await s.pause(500)
    s.say('It names the ticket before it posts, every time.')
    s.spotlight('[data-tour="wl-confirm"]')
    await s.point('[data-tour="wl-confirm"]')
    await s.waitForClick('[data-tour="wl-confirm"]', 120000)
    s.spotlight(null)
    await s.appeared('[data-tour="wl-posted"]', 8000)
    await s.pause(600)
    // AND IT SAYS THE POST WAS FAKE. Left unsaid, the tick is a lie the user
    // finds out about by opening their board - exactly the moment the product
    // most needs to have been straight with them.
    await s.next('Posted to the ticket, without opening your board. Example day, so nothing really went out.')
  } else {
    await s.next('Connect a board and it writes the ticket update too - matched, and posted once you approve.')
  }
  s.selectTask(null)
  await s.pause(500)

  // ── 6. The daily summary, SECTION BY SECTION ────────────────────────────
  // This used to be two lines over a card with a headline and three numbers on
  // it: open it, say "it says what you finished and what slipped", close it. The
  // screen it was describing is the product's actual end-of-day artefact - the
  // one thing a user would show someone else - and a wave at it taught nothing
  // about how to read it.
  //
  // So every section is named, in the order the eye takes them: the sentence, the
  // measured numbers, the two things worth remembering, the plan, the standup.
  // Five short beats rather than one long one, because a paragraph naming five
  // regions of a busy screen is a paragraph nobody maps onto the screen.
  // NO HANDOFF LINE, AND THE USER OPENS IT. This was a full-screen "Next" beat
  // ("that was one task, at the end of every day they roll up…") followed by the
  // tour clicking the pill ITSELF - so the user read a paragraph promising a
  // screen, pressed a button that was not the one being described, and then
  // watched the actual control get pressed for them. Two steps, neither of them
  // theirs.
  //
  // One ring, one short line beside it, and they press it. The sentence is the
  // label for the button under the cursor, not a preamble about what is coming:
  // the screen it describes is one click away and explains itself far better than
  // any promise about it could.
  // Says what the button IS, in the words on the button. "Every day rolls up
  // into one screen" describes a mechanism and makes the reader work out what
  // they are about to see; the pill under the cursor says "Daily summary", so
  // the line names that and gets out of the way.
  s.say('Your whole day, summarised. Open it.')
  s.spotlight('[data-tour="summary-pill"]')
  await s.point('[data-tour="summary-pill"]')
  await s.waitForClick('[data-tour="summary-pill"]', 60000)
  s.spotlight(null)
  // Opened HERE rather than by the pill: the tutorial toolbar is a replica and
  // its controls are deliberately inert (see TutorialScreen), so the script owns
  // the surface either way - the click is the user's, the render is ours. The
  // fallback path (they never clicked) lands here too, so the tour cannot stall
  // on a summary that never opens.
  s.demoSummary(true)
  await s.pause(900)

  s.spotlight('[data-tour="sum-hero"]')
  await s.next('The day in a sentence, first: how it went, and what got in the way.')
  // NO BEAT ON THE NUMBERS. They were rung and explained ("these are measured,
  // not written…"), which is a defence of a scoreboard nobody had questioned -
  // a donut and three counters read as measurements on sight, and stopping to
  // say so puts a paragraph between the sentence about the day and the two
  // things actually worth remembering from it. The provenance claim is made
  // where it does real work: on the WRITTEN parts, which are the ones a user has
  // any reason to doubt.
  s.spotlight('[data-tour="sum-cards"]')
  await s.next('Then the two things worth remembering: what pulled you off plan, and what you picked up doing it.')
  s.spotlight('[data-tour="sum-plan"]')
  await s.next('Your plan, ticked off against real work. The one you did not reach is carried to tomorrow, not lost.')
  s.spotlight('[data-tour="sum-standup"]')
  await s.next('And your standup, already written. Copy it straight into Slack in the morning.')
  s.spotlight(null)

  // ── 7. The work that was NOT on the plan — the OTHER worklog outcome ────
  // The first post, four beats ago, was the easy case: planned work, matched to
  // the ticket it was planned as. Every real day also contains the other kind -
  // the thing that came up - and how a tool handles THAT is what decides whether
  // it gets trusted. Meridian's answer is that it does not force it onto the
  // nearest half-match: it writes the ticket, and the user files it or overrules
  // it. Taught here rather than in the detail panel because this is where the
  // day's unplanned work is actually visible as a category.
  s.spotlight('[data-tour="sum-offplan"]')
  await s.next('Work that came up and matched nothing you planned - kept separately, not quietly dropped.')
  s.say('Open it.')
  s.spotlight('[data-tour="sum-offplan-row"]')
  await s.point('[data-tour="sum-offplan-row"]')
  await s.waitForClick('[data-tour="sum-offplan-row"]', 120000)
  s.spotlight(null)
  await s.pause(700)

  // NO "WHAT YOU DID" BEAT. That block is gone from the view: it bulleted the
  // same work the update below it describes in prose, and the beat on it had to
  // introduce the duplication before the next beat could explain it away. Its
  // one real claim - written from the work, not from a timer - has moved onto
  // the update, which is the part a new user has any reason to doubt.
  // IN READING ORDER, which is destination first - the beats follow the layout
  // rather than setting their own sequence, so the ring always moves downward.
  s.spotlight('[data-tour="sum-where"]')
  await s.next('Nothing on your plan covered this, so Meridian wrote the ticket for it. Approve, and it is created.')
  s.spotlight('[data-tour="sum-update"]')
  await s.next('And the update that goes on it, written from the work - not from a timer or a note.')
  s.spotlight(null)

  // 7a. THE PROPOSAL IS A SUGGESTION TOO. Shown as a control the user operates,
  // not mentioned in passing: someone who believes a drafted ticket is final
  // either stops trusting the drafts or stops using them, and both failures
  // happen quietly, weeks later, where no onboarding can reach.
  //
  // Guarded on the click, because everything inside it assumes the picker opened.
  // Either way the flow continues - a user who would rather not rummage through a
  // ticket list still gets the create-and-post ending.
  //
  // The anchors here are the DRAFT DIALOG'S OWN (`wl-*`), because this view is
  // now built from the dialog's own components rather than a replica of them -
  // the override and the primary are one row, exactly as they are on the real
  // surface. Unambiguous despite the detail panel using the same names: that
  // panel was closed by `selectTask(null)` above, taking its dialog with it.
  s.say('Or it belongs on a ticket you already have. That call is always yours - have a look.')
  s.spotlight('[data-tour="wl-rematch"]')
  await s.point('[data-tour="wl-rematch"]')
  if (await s.waitForClick('[data-tour="wl-rematch"]', 90000)) {
    s.spotlight(null)
    await s.pause(600)
    s.say('Every open ticket you have. Pick one and it files there instead - or cancel and let it create the new one.')
    s.spotlight('[data-tour="wl-picker"]')
    await s.waitForClick('[data-tour="wl-pick"]', 120000)
    await s.pause(600)
  }
  s.spotlight(null)

  // 7b. Create-and-post, then confirm - the same two presses the first worklog
  // took, because this is the same control doing the same outward-facing thing.
  // The button's own label changes with the choice above - it either creates a
  // ticket or files onto one - so the beat does not name which, and cannot
  // contradict what the user is looking at.
  s.say('Same rule as before: nothing leaves Meridian until you approve it.')
  s.spotlight('[data-tour="wl-approve"]')
  await s.point('[data-tour="wl-approve"]')
  await s.waitForClick('[data-tour="wl-approve"]', 120000)
  s.spotlight(null)
  await s.pause(500)
  s.spotlight('[data-tour="wl-confirm"]')
  await s.point('[data-tour="wl-confirm"]')
  await s.waitForClick('[data-tour="wl-confirm"]', 120000)
  s.spotlight(null)
  await s.appeared('[data-tour="wl-posted"]', 6000)
  await s.pause(600)
  await s.next('Filed and written up, from work that was never on a ticket. Example day - nothing went out.')

  // ── 8. Out of the summary, and WHEN it arrives ──────────────────────────
  // The summary has been read section by section, and the obvious question it
  // leaves ("do I have to come here and do that every evening?") is answered
  // before it is asked - and answered by the ask that follows it.
  await s.next('None of that is yours to build. Each evening Meridian writes it and tells you it is ready.')
  // CLOSED FOR THEM, straight back to the timeline. It used to ring the × and
  // wait for a click, which taught nothing - they opened this surface two beats
  // ago, so its way out is not news - and it put an errand between the summary
  // and the last stretch of the tour, at the point where the user is most ready
  // for it to end.
  s.demoSummary(false)
  await s.pause(700)

  // ── 9. AI ask — only when there is something to ask ──────────────────────
  // They have now read a task write-up and a whole summary a model wrote. THAT
  // is what makes this ask land, and why it cannot be moved earlier.
  //
  // NO BEAT AT ALL for anyone already connected. Both branches used to stop here
  // to say the writing was done by a model on their own machine and never
  // touches our servers - a claim that reads as a claim, arriving at the point
  // in the tour where the user is counting the beats left. Nothing is being
  // asked of them, so there is nothing to stop for.
  if (ai === null) {
    await s.next('Last thing: pick the AI that does the writing. It runs through your own CLI.', 'Pick a model')
    s.openSettings('intelligence')
    await s.pause(1000)
    s.say('Pick a provider - Meridian installs it and signs you in here. Change it any time.')
    await s.waitForClick('[data-tour="modal-close"]', 300000)
    s.openModal(null)
    await s.pause(500)
  }

  // ── 10. When does the day end — the one setting, asked last ──────────────
  // The REAL dialog (`WorklogAutoGenerateDialog`), and the time is really saved.
  // Ordinarily the shell raises it by itself on a first Timeline load, which
  // during a walkthrough means over the title sequence - so it is suppressed
  // there and placed here instead, where the user has just watched exactly the
  // thing this time sets off.
  await s.next(
    'One question left, and it is the only thing Meridian will ask you to set: what time do you want your summary and drafts ready?',
    'Set the time', { center: true })
  s.showWorklogSchedule(true)
  await s.pause(800)
  // NO RING WHILE THIS DIALOG IS UP. A spotlight anchors the caption to the
  // thing it rings, and everything worth ringing here is INSIDE a dialog that
  // sits in the middle of the window - so the caption landed on top of the copy
  // the user is being asked to read. Without one it renders in the top bar,
  // clear above the dialog, and the cursor still does the pointing.
  s.say('Meridian has them written by then and tells you they are waiting. Change it any time.')
  await s.pause(1400)
  await s.point('[data-tour="worklog-schedule-on"]')
  // Resolves on the click OR on the dialog leaving the DOM, which covers "Not
  // now" and the backdrop - both are the question being answered, and neither
  // should strand the tour behind a dialog that is no longer there.
  await s.waitForClick('[data-tour="worklog-schedule-on"]', 300000)
  s.spotlight(null)
  s.showWorklogSchedule(false)
  await s.pause(600)

  // ── 11. Running, and you can stop looking at it ─────────────────────────
  // The reassurance the whole product depends on and the one a demo cannot
  // demonstrate: nothing more is required of them. Said plainly, with the
  // notification promise attached, because the alternative reading of an ambient
  // tool is that it needs minding.
  await s.next(
    "You are set up. Close this window - Meridian keeps going, and tells you when something is ready.",
    'One last thing', { center: true })

  // ── 12. The off switch ──────────────────────────────────────────────────
  // Taught last and taught deliberately. An ambient recorder that never shows
  // you how to stop it is a thing you switch off at the operating system, and
  // the tray is where that lives - outside this window, so the tour draws it (see
  // `TutorialTray`). Nothing here pauses anything, and the closing line says so:
  // a walkthrough that quietly stopped capture on its way past would leave a new
  // user with a product that does not work.
  s.demoTray(true)
  await s.pause(600)
  // ONE NAME, TWO DIRECTIONS. It is "the tray" on both platforms - that is what
  // the app calls it everywhere else, and a user who has just been told about
  // their "menu bar" has to translate before they can look. What DOES change is
  // where to look: top-right on macOS, bottom-right on Windows, so the direction
  // word is platform-driven and the noun is not. The replica draws itself in the
  // matching corner (`TutorialTray`), and the line follows it.
  s.say(trayIsAtTheTop()
    ? 'Meridian is up here in your tray, always. Click it.'
    : 'Meridian is down here in your tray, always. Click it.')
  s.spotlight('[data-tour="tray-icon"]')
  await s.point('[data-tour="tray-icon"]')
  await s.waitForClick('[data-tour="tray-icon"]', 90000)
  s.spotlight(null)
  await s.pause(700)

  s.say('This is where you stop it - any time, for any reason, no explanation needed.')
  s.spotlight('[data-tour="tray-pause"]')
  await s.point('[data-tour="tray-pause"]')
  await s.waitForClick('[data-tour="tray-pause"]', 90000)
  s.spotlight(null)
  await s.pause(600)

  s.say('Fifteen minutes, an hour, until tomorrow, or until you turn it back on. Take the first one.')
  s.spotlight('[data-tour="tray-15m"]')
  await s.point('[data-tour="tray-15m"]')
  await s.waitForClick('[data-tour="tray-15m"]', 90000)
  s.spotlight(null)
  await s.pause(800)
  await s.next('Capture stops at once, and restarts itself when the time is up. Nothing was really paused.')
  s.demoTray(false)
  await s.pause(400)

  // ── 13. Handoff ─────────────────────────────────────────────────────────
  // The dangerous moment: the example day disappears and the real one is empty
  // except for the setup card. Naming that plainly is the whole job here - an
  // unexplained empty screen after a polished demo reads as the product being
  // broken. The promise made is deliberately event-shaped ("as you work") rather
  // than clock-shaped: the hourly fold only runs for hours clearing its activity
  // gate, so no specific time can be promised without risking a broken one.
  s.showExample(false)
  await s.pause(400)
  // THE ONE CELEBRATION, and it lives here. It used to fire mid-tour, at the seam
  // where the user stops giving Meridian things and starts being shown what it
  // does back (see `script.ts`, beat 2f) - a real milestone, but not the finish
  // line, and a tour that throws confetti twice has thrown it at nothing. This is
  // the last screen of a first run, and the only moment where "you are done" is
  // literally true.
  //
  // AND THE LINE IS ADDRESSED TO THEM, not to the product. Every other beat in
  // the tour describes what Meridian does; this one is the only place it says why
  // it was built, and "your work stops going unnoticed" is the whole of it.
  await s.next(
    'That is everything. Meridian runs from your tray now - here is hoping it makes a difference, and that your work stops going unnoticed.',
    'Finish', { center: true, celebrate: true })
  s.say('')
}
