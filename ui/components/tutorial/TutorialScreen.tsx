//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The walkthrough's surface: an OPAQUE layer that replaces the dashboard for the
// duration of the tour.
//
// Why replace rather than decorate. The first version narrated over the user's
// live dashboard, which failed in both directions at once: anyone with real work
// on screen had their actual tasks used as demo props ("this one was worked in
// two sittings" pointing at a card about their own bug), and anyone with a fresh
// install got a blank screen for the half of the script that needs a populated
// day. Owning the surface fixes both — the walkthrough shows exactly the day it
// is describing, and the user's real data is never spoken over.
//
// It is a sibling of the shell, not a fork of it: the timeline column is the
// REAL `DayTaskColumn` (fed through the injected-`tasks` prop it already
// supports for the LLM Lab), and the charts are the real ones. Only the chrome
// and the right panel are tutorial-owned, because those are the parts that would
// otherwise have needed demo branches inside product components.
//
// Layered BELOW the shell's modals (z-30 vs their z-40) on purpose: the beats
// that open the real planner and real Settings must show them on top of this.
//
// # Who calls this
// [`useTutorial`], which returns it as `screen`; the shell just renders it.
//
// # Related
// - `./TutorialPanel.tsx` — the right panel
// - `./TutorialSummaryCard.tsx` — the end-of-day summary a beat opens
// - `./sampleDay.ts` — the example day
// - `./script.ts` — the beats that drive `phase` / `summaryOpen`

import { DayTaskColumn } from '@/components/timeline/DayTaskColumn'
import { DayTaskDetailPanel, type DayTaskDetail } from '@/components/timeline/DayTaskDetailPanel'
import { TOUR_SURFACE_ATTR } from './engine'
import { TutorialPanel } from './TutorialPanel'
import { ReplayAside } from './ReplayAside'
import { demoBoardTickets, useDemoWorklog } from './demoWorklog'
import { TutorialSummaryCard } from './TutorialSummaryCard'
import { sampleDayString, type SampleOverview } from './sampleDay'
import type { DayTask } from '@/lib/api-types'

/** Today as `YYYY-MM-DD`, for the empty phase (which is presented as the user's
 *  own day-in-progress rather than a dated example). */
function todayString(): string {
  const d = new Date()
  const p = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`
}

export function TutorialScreen({ phase, tasks, replayMinute, replayNote = null, sample, selected, onSelect, summaryOpen, provider, onOpenPlan }: {
  phase: 'empty' | 'example'
  /** The cards to draw — empty in the first phase. */
  tasks: DayTask[]
  /** Minutes past 9 AM while `Stage.replayDay` runs, else null. Two jobs: it
   *  drives the timeline's own now-line and lit hour rail (`replayNowMin`), and
   *  it swaps the right panel out - during the replay the panel's own stats would
   *  be a finished day's totals sitting beside a timeline still filling, i.e. the
   *  answer printed next to the question. */
  replayMinute: number | null
  /** The replay's current narration line, shown in the right panel (not the
   *  overlay bar, which sits on top of the timeline). */
  replayNote?: string | null
  sample: SampleOverview
  selected: DayTaskDetail | null
  onSelect: (d: DayTaskDetail | null) => void
  summaryOpen: boolean
  /** The tracker the user connected during the tour, so the summary's post flow
   *  names their board instead of promising Jira to a GitHub user. */
  provider: { id: string; name: string } | null
  onOpenPlan: () => void
}) {
  const isExample = phase === 'example'
  // The scripted worklog machine behind the REAL detail panel. Built here (rather
  // than inside the panel) because it must survive the panel unmounting when the
  // user clicks away from the card and back - the same reason the live hook keeps
  // its state in a module store.
  const demoWl = useDemoWorklog(provider?.id ?? 'jira')
  const demoTickets = demoBoardTickets(provider?.id ?? 'jira')
  return (
    // `data-tour-surface`: this is a REPLICA drawn OVER the live dashboard, not
    // a replacement for it - the real shell is still mounted underneath with the
    // same `data-tour` hooks on its own toolbar and cards, and it renders first,
    // so an unscoped `querySelector` finds the hidden original every time. The
    // marker lets `tourTarget` prefer what is actually on screen. Without it the
    // summary beat rang a spot beside the pill and could only ever end on its
    // fallback timer, because the button the user could press was never measured.
    <div {...{ [TOUR_SURFACE_ATTR]: '' }}
      className="absolute inset-0 flex flex-col" style={{ zIndex: 30, background: 'var(--win-bg)' }}>
      <TutorialToolbar isExample={isExample} />

      {/* data-tour: the beat right after the replay rings the WHOLE view at once -
          the timeline and the glance panel together - because the point being made
          there is about the day as a finished thing, not about any one card. */}
      <div data-tour="day-view" className="flex flex-1 min-h-0">
        {/* No replay class here any more. This used to carry `mer-replay`, whose
            rule animated every `.dt-card` on MOUNT - which worked only because
            the walkthrough was feeding the column a task list that grew, so each
            reveal was a fresh mount. That list is exactly what made the replay
            look wrong (the column re-scaled itself around each new card), and
            with the whole day handed over up front nothing mounts mid-replay, so
            the mount hook has nothing to fire on. The reveal now lives on the
            card, driven by `replayNowMin` - see `revealed` in DayTaskColumn.

            The old keyframe also scaled the card up from .96, which read as the
            view zooming on every card. The replacement only fades and rises: at
            replay speed a scale change on a card that is already in its final
            position is indistinguishable from the layout moving. */}
        <div className="relative flex-1 min-w-0 min-h-0 flex flex-col">
          <DayTaskColumn
            day={isExample ? sampleDayString() : todayString()}
            // Only the empty phase is "today": it is the user's own day, and the
            // live-hour strip saying Meridian is watching this hour is exactly
            // the reassurance that phase is built around. The example is a past
            // day and must not claim a live hour.
            isToday={!isExample}
            tasks={tasks}
            selectedId={selected?.id ?? null}
            onSelect={onSelect}
            // The replay's clock, on the rail where the work is - see ReplayAside
            // for why it is not a dial in the panel any more. Absolute minutes
            // from midnight; `replayMinute` counts from 9 AM.
            replayNowMin={replayMinute !== null ? 540 + replayMinute : null}
          />
        </div>
        {/* Keyed on WHICH view is in the slot, so React remounts on a change and
            the pane fades up instead of being swapped out from under the user.
            Three unrelated things share this column across the run - the replay
            clock, a task write-up, the standing panel - and every handover
            between them was a hard cut, which is what made the tour feel like a
            slideshow rather than one screen being operated. Not keyed on the
            task id: re-selecting a card must not re-animate a panel that is
            already there.

            The rise is safe here ONLY because the worklog draft dialog portals
            to `document.body`. It is `fixed inset-0` so it can centre on the
            window from in here, and while it was a DOM descendant of this
            column both halves of this animation broke it - the transform made
            this div the containing block for `fixed` (dialog fenced into 388px),
            and the opacity opened a stacking context (timeline painting over
            it). Anything else `fixed` rendered into this slot needs the same
            treatment; see the header of `WorklogDraftDialog`. */}
        <div key={replayMinute !== null ? 'replay' : selected ? 'task' : 'panel'}
          className="shrink-0 border-l min-h-0 mer-tour-pane"
          style={{ width: 388, borderColor: 'var(--t-hair)', background: 'var(--t-panel)' }}>
          {replayMinute !== null
            ? <ReplayAside minute={replayMinute} note={replayNote} />
            : selected
              ? <DayTaskDetailPanel detail={selected} onClose={() => onSelect(null)}
                  onCorrected={() => onSelect(null)} onOpenSettings={() => {}} onOpenTask={() => {}}
                  worklog={demoWl} boardTickets={demoTickets} />
              : <TutorialPanel sample={sample} phase={phase} onOpenPlan={onOpenPlan} />}
        </div>
      </div>

      {summaryOpen && <TutorialSummaryCard sample={sample} provider={provider} />}
    </div>
  )
}

/** A stand-in for the real Toolbar. Deliberately inert: every control up there
 *  (day arrows, nav pills, Settings) would take the user somewhere the script is
 *  not expecting. The one thing it carries is the Daily-summary button, which a
 *  beat points at, opens, and spends the whole back half of the tour inside.
 *
 *  IT SITS WHERE THE REAL ONE SITS - left, immediately after the day label - and
 *  wears the real one's treatment (see `Toolbar.tsx`). It used to be a faint
 *  `<span>` pushed to the far right of an otherwise empty strip, which failed
 *  twice over: it did not read as a control at all, so the beat that flies a
 *  cursor into it and clicks looked like the summary opening by itself; and it
 *  taught the wrong place, so the first thing the user does in the real app after
 *  the tour is hunt the top-right corner for a button that is on the left. */
function TutorialToolbar({ isExample }: { isExample: boolean }) {
  return (
    <div className="relative flex items-center gap-4 px-5 border-b shrink-0"
      style={{ height: 60, borderColor: 'var(--t-hair)' }}>
      {/* CENTRED, and absolutely so. Which day is on screen is the one fact that
          orients everything else in the tour - it is what tells the user the
          populated day is an EXAMPLE and not their own - and in the left slot it
          read as one more label in a row of controls. Taken out of the flow so
          it centres on the WINDOW rather than on whatever is left over after the
          pill, which would move it every time the pill's label changed. */}
      <p className="mt-toolbar-date absolute left-1/2 -translate-x-1/2 pointer-events-none"
        style={{ color: 'var(--t-title)' }}>
        {isExample ? 'An example day' : 'Today'}
      </p>
      <button data-tour="summary-pill"
        className="inline-flex items-center gap-1.5 rounded-full px-3 py-1.5 bg-ctrl shrink-0"
        style={{ border: '1px solid var(--t-ctrl-border)', cursor: 'pointer' }}>
        <span aria-hidden="true" className="inline-block rounded-full"
          style={{ width: 6, height: 6, background: 'var(--accent)' }} />
        <span className="mt-body-sm" style={{ color: 'var(--t-muted)' }}>Daily summary</span>
      </button>
    </div>
  )
}
