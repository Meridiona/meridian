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
// - `./sampleDay.ts` — the example day
// - `./script.ts` — the beats that drive `phase` / `summaryOpen`

import { DayTaskColumn } from '@/components/timeline/DayTaskColumn'
import { DayTaskDetailPanel, type DayTaskDetail } from '@/components/timeline/DayTaskDetailPanel'
import { TutorialPanel } from './TutorialPanel'
import { sampleDayString, type SampleOverview } from './sampleDay'
import type { DayTask } from '@/lib/api-types'

/** Today as `YYYY-MM-DD`, for the empty phase (which is presented as the user's
 *  own day-in-progress rather than a dated example). */
function todayString(): string {
  const d = new Date()
  const p = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`
}

export function TutorialScreen({ phase, tasks, sample, selected, onSelect, summaryOpen, onOpenPlan }: {
  phase: 'empty' | 'example'
  /** The cards to draw — empty in the first phase. */
  tasks: DayTask[]
  sample: SampleOverview
  selected: DayTaskDetail | null
  onSelect: (d: DayTaskDetail | null) => void
  summaryOpen: boolean
  onOpenPlan: () => void
}) {
  const isExample = phase === 'example'
  return (
    <div className="absolute inset-0 flex flex-col" style={{ zIndex: 30, background: 'var(--win-bg)' }}>
      <TutorialToolbar isExample={isExample} />

      <div className="flex flex-1 min-h-0">
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
          />
        </div>
        <div className="shrink-0 border-l min-h-0"
          style={{ width: 388, borderColor: 'var(--t-hair)', background: 'var(--t-panel)' }}>
          {selected
            ? <DayTaskDetailPanel detail={selected} onClose={() => onSelect(null)}
                onCorrected={() => onSelect(null)} onOpenSettings={() => {}} onOpenTask={() => {}} />
            : <TutorialPanel sample={sample} phase={phase} onOpenPlan={onOpenPlan} />}
        </div>
      </div>

      {summaryOpen && <TutorialSummary sample={sample} />}
    </div>
  )
}

/** A stand-in for the real Toolbar. Deliberately inert: every control up there
 *  (day arrows, nav pills, Settings) would take the user somewhere the script is
 *  not expecting. The one thing it carries is the Daily-summary pill, which a
 *  beat points at. */
function TutorialToolbar({ isExample }: { isExample: boolean }) {
  return (
    <div className="flex items-center gap-3 px-5 py-3.5 border-b shrink-0"
      style={{ borderColor: 'var(--t-hair)' }}>
      <p className="mt-greeting text-title" style={{ fontSize: 17 }}>
        {isExample ? 'An example day' : 'Today'}
      </p>
      <span className="flex-1" />
      <span data-tour="summary-pill"
        className="inline-flex items-center gap-2 rounded-full px-3.5 py-1.5"
        style={{ background: 'var(--t-card)', border: '1px solid var(--t-card-border)' }}>
        <span className="rounded-full" style={{ width: 6, height: 6, background: 'var(--color-state-proposal)' }} />
        <span className="mt-body-sm" style={{ color: 'var(--t-title)' }}>Daily summary</span>
      </span>
    </div>
  )
}

/** The end-of-day summary, as the marketing demo renders it (`renderSummary` in
 *  demo.js). Its own component rather than the real `DaySummaryOverlay`, which
 *  reads the user's actual day — on a fresh install that is empty, and on a
 *  working one it is their real work, which is the thing this whole screen
 *  exists to stop showing. Copy is carried over verbatim. */
function TutorialSummary({ sample }: { sample: SampleOverview }) {
  const done = sample.focusItems.filter(t => t.is_terminal).length
  return (
    <div className="absolute inset-0 flex items-center justify-center p-10"
      style={{ zIndex: 5, background: 'color-mix(in srgb, var(--win-bg) 82%, transparent)', backdropFilter: 'blur(3px)' }}>
      <div className="w-full rounded-2xl p-8 bg-card mer-pop"
        style={{ maxWidth: 620, border: '1px solid var(--t-card-border)', boxShadow: 'var(--mt-modal-shadow)' }}>
        <p className="mt-label" style={{ color: 'var(--color-state-proposal)' }}>Daily summary</p>
        <p className="mt-greeting text-title mt-2" style={{ fontSize: 24 }}>Here&apos;s your day</p>
        <p className="mt-body mt-3" style={{ color: 'var(--t-muted)', lineHeight: 1.55 }}>
          You wrapped <b style={{ color: 'var(--t-title)' }}>{done} of {sample.focusItems.length}</b> planned
          tasks. An urgent logout bug pulled you off the third - it rolls to tomorrow, already noted.
        </p>
        <div className="grid grid-cols-3 gap-3 mt-6">
          <SummaryStat n={`${done} / ${sample.focusItems.length}`} l="Planned done" />
          <SummaryStat n={fmtHrs(sample.engagedSeconds)} l="Time logged" />
          <SummaryStat n="+1" l="Urgent pickup" accent />
        </div>
      </div>
    </div>
  )
}

function SummaryStat({ n, l, accent }: { n: string; l: string; accent?: boolean }) {
  return (
    <div className="rounded-xl p-3.5 bg-box text-center">
      <p className="mt-stat" style={{ color: accent ? 'var(--color-state-proposal)' : 'var(--t-title)' }}>{n}</p>
      <p className="mt-label mt-1" style={{ color: 'var(--t-faint)' }}>{l}</p>
    </div>
  )
}

function fmtHrs(secs: number): string {
  const m = Math.round(secs / 60)
  const h = Math.floor(m / 60)
  return h > 0 ? `${h}h ${m % 60}m` : `${m}m`
}
