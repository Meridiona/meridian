//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

// The example day the first-run walkthrough renders through the REAL timeline.
//
// Why this exists: Meridian is ambient, so a brand-new user's timeline is
// structurally empty. Day-task cards are produced by the hourly fold
// (`workstream::run`, called from the HH:03 pipeline) and that fold only runs for
// hours clearing the activity gate — more than 5 sessions over 15s each, or any
// coding-agent session. A user who finishes setup and reads the dashboard for ten
// minutes clears neither, so the main surface of the product can stay blank for
// hours. Teaching the UI on that blank surface is teaching nothing.
//
// So the walkthrough injects this day via DayTaskColumn's `tasks` prop, which
// bypasses the DB fetch and the 30s poll entirely (see DayTaskColumn's
// `if (tasks) { … return }`) — the user learns on the real component, with real
// interactions, against a populated day.
//
// The content is ported from the public marketing demo
// (meridiona-website/assets/js/demo.js `initialState()`), which was itself built
// against live screenshots of this dashboard. Reusing it means the product shows
// the same day the website promised, and the copy has already been reviewed for
// public display. It is remapped here because the demo's shape is bespoke
// (`[[startMin,endMin]]` offsets from 9 AM, `hue`/`cat`/`status`) and the real
// component wants `DayTask`/`DaySegment` (`"HH:MM"` strings, `hours` labels,
// `linked_ticket`, `posted_provider`).
//
// # Who calls this
// [`useTutorial`] — passes `sampleTasks` to `DayTaskColumn` while the walkthrough
// is running, and drops it at handoff so the real (possibly empty) day returns.
//
// # Related
// - `ui/lib/api-types.ts` — the `DayTask`/`DaySegment` contract this must satisfy
// - `src/worklog_pipeline/task_db.rs` — `SETUP_TASK_ID`, the one REAL card the
//   walkthrough writes at handoff so the timeline is never empty afterwards

import type { DayTask, DaySegment, PlanItem, TodaySession } from '@/lib/api-types'

/** `YYYY-MM-DD` for the example day: yesterday, local.
 *
 *  Deliberately not today. The example runs to 17:20, so on a mid-afternoon
 *  install "today" would show hours that have not happened yet — work the user
 *  can see they did not do. A past day reads correctly at any install time, and
 *  the hour rail still looks like a normal working day. The overlay labels it as
 *  an example regardless; this only removes the second, subtler wrongness. */
export function sampleDayString(): string {
  const d = new Date()
  d.setDate(d.getDate() - 1)
  const p = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`
}

/** Minutes-from-midnight → local `"HH:MM"`. */
function hhmm(min: number): string {
  const p = (n: number) => String(n).padStart(2, '0')
  return `${p(Math.floor(min / 60))}:${p(min % 60)}`
}

/** Build a task from minute-offset segment pairs, deriving every field the real
 *  `DayTask` carries redundantly (`minutes`, `hours`, `first_hour`, `last_hour`)
 *  rather than hand-writing them — those are computed deterministically in the
 *  Rust fold, and hand-copied values would drift from the segments they describe. */
function task(
  id: string,
  title: string,
  pairs: [number, number][],
  summary: string[],
  ticket: string | null,
  posted: { provider: string; key: string; url: string } | null,
  day: string,
): DayTask {
  const segments: DaySegment[] = pairs.map(([s, e]) => ({ start: hhmm(s), end: hhmm(e) }))
  const minutes = pairs.reduce((a, [s, e]) => a + (e - s), 0)
  const hourSet = new Set<number>()
  for (const [s, e] of pairs) {
    for (let h = Math.floor(s / 60); h <= Math.floor((e - 1) / 60); h++) hourSet.add(h)
  }
  const hoursNum = [...hourSet].sort((a, b) => a - b)
  return {
    id,
    title,
    summary,
    minutes,
    hours: hoursNum.map((h) => `${day}T${String(h).padStart(2, '0')}`),
    segments,
    first_hour: hoursNum[0] ?? -1,
    last_hour: hoursNum[hoursNum.length - 1] ?? -1,
    status: 'active',
    linked_ticket: ticket,
    posted_provider: posted?.provider ?? null,
    posted_target_key: posted?.key ?? null,
    posted_browse_url: posted?.url ?? null,
  }
}

/** The example day, ordered as the timeline lays it out.
 *
 *  The mix is deliberate and worth preserving if this is ever edited:
 *  - `bug` has TWO segments — the card that proves work scattered across a
 *    morning gets folded into one task. It is the walkthrough's click target.
 *  - `slack` and `talk` carry NO ticket. This is the quiet but important one: it
 *    teaches, using a card rather than a sentence, that Meridian does not force
 *    everything into a ticket. That is the expectation users most need set early,
 *    and the fear ("it'll try to bill my whole day to something") most likely to
 *    lose them. */
export function sampleTasks(): DayTask[] {
  const day = sampleDayString()
  return [
    // NO "intro" TASK HERE. An attempt to give the replay an opening line put it
    // in this list as a fake task, which made it a real card in the example day:
    // counted in "N tasks today", added to the tracked total, clickable, and
    // still sitting there for
    // every beat after the replay that points at a card. The example day must
    // contain only work.
    task('s1', 'Catching up on Slack and inbox messages',
      [[540, 575]],
      [
        'Caught up on 14 messages that piled up overnight across the team.',
        'Read through everything to see what needed a reply first.',
      ],
      null, null, day),

    // Carries a ticket but is NOT posted. The marketing demo showed this one
    // already synced to Jira; here it would draw a "synced to Jira" pill on a
    // card belonging to a user who has not connected a tracker yet — the
    // walkthrough asks them to do that two beats later. Showing the end state
    // before the ask makes the ask look redundant.
    task('s2', 'Team standup & planning the week ahead',
      [[585, 630]],
      [
        "Updated the team on yesterday's progress fixing a login bug.",
        "Reviewed the task list and estimated what's next.",
      ],
      'MER-501', null, day),

    // NO TICKET, and that is the point of this card. It is the day's genuinely
    // unplanned work - nothing on the morning's plan covers it - so it is what
    // the summary uses to teach the OTHER worklog outcome: work that matched
    // nothing, for which Meridian drafts a brand-new ticket rather than forcing
    // it onto the nearest wrong one. Giving it a key here would quietly make
    // that beat a lie (the ticket would already exist).
    task('s3', 'Fixing a bug that randomly logged people out',
      [[640, 695], [705, 755]],
      [
        'Figured out why some users were getting logged out unexpectedly.',
        "Fixed the underlying timing issue so it can't happen again.",
        'Opened a draft fix for a teammate to double-check before it ships.',
      ],
      null, null, day),

    task('s4', "Reviewing a teammate's code change",
      [[735, 775]],
      [
        'Left detailed feedback on how old data gets cleared out over time.',
        'Caught an edge case that could cause a rare double-cleanup.',
      ],
      'MER-495', null, day),

    task('s5', 'Making the activity feed feel instant',
      [[820, 910], [930, 1040]],
      [
        'Found why the feed was taking a while to show up for people with a lot of activity.',
        'Changed how it loads so it shows up almost instantly, no matter how much history someone has.',
        'Double-checked it worked well for small, medium, and very active accounts before shipping.',
        'Shipped it - the feed now feels noticeably snappier for everyone.',
      ],
      'MER-475', null, day),

    task('s6', 'Watching a talk on YouTube while a long test run finished',
      [[830, 1030]],
      [
        'Kept a systems-design talk playing in the background while the test suite ran.',
        'Picked up a couple of ideas worth trying on the activity-feed work.',
      ],
      null, null, day),
  ]
}

// ── The right panel's half of the example day ────────────────────────────────
// The walkthrough teaches the WHOLE screen, so the right panel has to be part
// of the example too. Left live it would sit beside a populated demo timeline
// showing "0m logged", an empty Time-by-app and an empty focus list — the exact
// blankness the example day exists to avoid, half a screen away from it.
//
// The numbers are the marketing demo's own (`renderPanel` in demo.js), not
// re-derived here. Three of them are computed differently from one another on
// purpose, and getting that wrong is what made an earlier pass show a wrong
// Focus figure:
//
//   Focus         unionMinutes(dayTasks) — the day's WALL-CLOCK footprint, so
//                 overlapping work counts once. Two of the example tasks run
//                 concurrently (a talk playing while a test suite ran), so
//                 summing task minutes instead overstates the day by ~3 hours.
//   Time by app   its OWN hardcoded per-app minutes. Deliberately not derived
//                 from the tasks: apps and tasks are different cuts of the day
//                 and one is not reconstructible from the other.
//   Time by category  per-task `cat` × taskMinutes (a plain SUM). A donut shows
//                 distribution across a set, so double-counted concurrent time
//                 is correct there in a way it is not for Focus.
//
// So the donut total (10h 25m) is legitimately larger than Focus (7h 5m). That
// is the same relationship the real app has between category time and
// `engaged_s`; do not "fix" it by making them agree.

/** Per-app minutes, verbatim from the demo's `appMinutes`. */
const APP_MINUTES: Record<string, number> = {
  'Claude Code': 148,
  'Google Chrome': 96,
  GitHub: 74,
  Slack: 62,
  'System Settings': 12,
}

/** Each example task's category, from the demo's `cat` field. */
const TASK_CATS: Record<string, string> = {
  s1: 'communication',
  s2: 'meeting',
  s3: 'coding',
  s4: 'code_review',
  s5: 'coding',
  s6: 'research',
}

/** Base fields shared by every synthetic session, so the two builders below
 *  differ only in the one dimension each is about. */
function session(id: number, day: string, app: string, cat: string, durSec: number): TodaySession {
  return {
    id, app, cat,
    started_at: `${day}T09:00:00`,
    dur: durSec,
    titles: [], explain: null, routing: null, session_type: null,
    task_key: null, candidates: [], confidence: 0, method: 'sample',
    link_method: null, link_confidence: null, summary: null,
  }
}

/** Sessions carrying ONLY app time — `cat` is blank so `categoryTotals` skips
 *  them entirely. Shaped as real `TodaySession`s (rather than pre-aggregated
 *  rows) so the panel runs its own `appTotals` over them exactly as it does on
 *  a real day. Nothing here is persisted or sent anywhere. */
export function sampleAppSessions(): TodaySession[] {
  const day = sampleDayString()
  return Object.entries(APP_MINUTES).map(([app, min], i) => session(i + 1, day, app, '', min * 60))
}

/** Sessions carrying ONLY category time — `app` is blank so `appTotals` skips
 *  them. Kept a separate list from the app sessions because the two are
 *  independent cuts of the day (see the block comment above); one array
 *  feeding both charts would force them into an agreement neither the demo nor
 *  the real app has. */
export function sampleCatSessions(): TodaySession[] {
  const day = sampleDayString()
  return sampleTasks().map((t, i) => session(i + 1, day, '', TASK_CATS[t.id] ?? 'coding', t.minutes * 60))
}

/** The day's wall-clock footprint in minutes: every task's segments merged, so
 *  concurrent work is counted once. Port of the demo's `unionMinutes`. */
function unionMinutes(): number {
  const segs: [number, number][] = []
  for (const t of sampleTasks()) {
    for (const s of t.segments) {
      const [sh, sm] = s.start.split(':').map(Number)
      const [eh, em] = s.end.split(':').map(Number)
      segs.push([sh * 60 + sm, eh * 60 + em])
    }
  }
  segs.sort((a, b) => a[0] - b[0])
  let total = 0
  let curStart: number | null = null
  let curEnd = 0
  for (const [s, e] of segs) {
    if (curStart === null) { curStart = s; curEnd = e; continue }
    if (s <= curEnd) { curEnd = Math.max(curEnd, e); continue }
    total += curEnd - curStart
    curStart = s
    curEnd = e
  }
  if (curStart !== null) total += curEnd - curStart
  return total
}

/** The example day's "Today's focus" checklist, ported from the marketing
 *  demo's `tasks` array. Two done, one not — the third is deliberately the one
 *  no timeline card matches, because a plan that came out exactly as written is
 *  not what a real day looks like. */
export function samplePlanItems(): PlanItem[] {
  const item = (
    key: string, position: number, title: string, done: boolean, type: string,
  ): PlanItem => ({
    task_key: key,
    position,
    origin: 'sample',
    title,
    provider: 'jira',
    url: '',
    status: done ? 'Done' : 'In Progress',
    is_terminal: done,
    due_date: null,
    due_days: null,
    description: '',
    epic: null,
    priority: null,
    issue_type: type,
    story_points: null,
  })
  return [
    item('MER-501', 0, "Share today's standup notes with the team", true, 'Task'),
    item('MER-475', 1, 'Ship the activity-feed pagination', true, 'Story'),
    item('MER-510', 2, 'Polish onboarding empty states', false, 'Task'),
  ]
}

/** Everything the Overview panel needs to render the example day.
 *
 *  `greetingBody` is supplied rather than computed by the panel because the
 *  panel's live version leads with time LOGGED, and the example has nothing
 *  logged: no card is posted, since the user has not been asked to connect a
 *  tracker yet (that comes two beats later). "0m logged across 0 work logs"
 *  would be the first sentence they read. Leading with focused time and the
 *  waiting drafts says the same thing about the same day, truthfully. */
export function sampleOverview() {
  const tasks = sampleTasks()
  const focusMinutes = unionMinutes()
  const appCount = Object.keys(APP_MINUTES).length
  // Cards with a ticket but not yet posted — everything with a ticket, since
  // nothing in the example is posted.
  const pendingCount = tasks.filter((t) => t.linked_ticket && !t.posted_provider).length
  return {
    appSessions: sampleAppSessions(),
    catSessions: sampleCatSessions(),
    engagedSeconds: focusMinutes * 60,
    focusItems: samplePlanItems(),
    pendingCount,
    greetingBody:
      `${fmtSampleDur(focusMinutes)} of focused activity across ${appCount} apps.`
      + ` ${pendingCount} draft${pendingCount === 1 ? '' : 's'} waiting for your review.`,
  }
}

/** `fmtDur`-compatible minutes formatter, kept local so this module stays free
 *  of component imports. */
function fmtSampleDur(min: number): string {
  const h = Math.floor(min / 60)
  const m = min % 60
  if (h <= 0) return `${m}m`
  return m ? `${h}h ${m}m` : `${h}h`
}

export type SampleOverview = ReturnType<typeof sampleOverview>

/** The card the walkthrough points at when it teaches multi-sitting folding AND
 *  then drives all the way through Generate → match → approve → posted.
 *
 *  ONE CARD FOR BOTH ON PURPOSE. It is the only one that satisfies both: two
 *  segments with a break between them (so the folding claim is visible), and a
 *  ticket that is on the day's plan (so the match beat can show a strong,
 *  honest 90% against something the user planned this morning). Splitting the
 *  two beats across two cards meant opening a second panel mid-flow and
 *  re-establishing what the user was looking at.
 *
 *  Named rather than indexed so re-ordering the list above cannot silently
 *  retarget the script at the wrong card. */
export const FOCUS_TASK_ID = 's5'

/** Alias kept for the summary, which draws its worklog rows from the same card. */
export const DRAFT_TASK_ID = FOCUS_TASK_ID

/** The day's unplanned work — no ticket, on no plan row. The summary uses it to
 *  teach the second worklog outcome: nothing matched, so Meridian drafts a NEW
 *  ticket and the user creates it (or overrides and matches an existing one). */
export const OFFPLAN_TASK_ID = 's3'
