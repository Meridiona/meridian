//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The walkthrough's end-of-day summary — the screen the whole second half of the
// tour builds toward, and the one place the product's output is shown whole.
//
// NOT the real `DaySummaryOverlay`. That one reads the user's actual day, which
// on a fresh install is empty and on any other is their genuine work — the exact
// thing the example day exists to avoid putting words in front of. This is the
// marketing demo's summary (`summaryHomeBody` / `summaryTaskBody` in
// meridiona-website/assets/js/demo.js), ported section for section so the product
// shows the same screen the website promised, in copy already reviewed for public
// display. Every em-dash in the source is a plain hyphen here, per the house rule
// on user-facing text.
//
// TWO VIEWS, one card. This file is the home view — the DAY — and it holds the
// state both share; [`TaskView`] is one strand of it, opened by clicking a row.
// The swap is internal state rather than a script-driven prop because the tour
// hands these clicks to the USER: the beats only point, wait, and narrate,
// exactly as they do over the real product.
//
// The plan section carries an OFF-PLAN block under it, and that is not padding.
// A real day contains work nobody planned, and a summary that quietly folds it
// into the plan (or drops it) is the one people stop trusting — it is precisely
// the work they get asked about and cannot account for. It is also the only
// honest place to teach the second worklog outcome: those strands matched no
// ticket, so Meridian drafts a new one rather than forcing them onto the nearest
// half-match. The plan rows themselves are therefore NOT clickable; the row the
// tour opens is an off-plan one.
//
// # Who calls this
// [`TutorialScreen`], while `Stage.demoSummary(true)` is in effect.
//
// # Related
// - `./TutorialSummaryTask.tsx` — the task view, and the create-or-match flow
// - `./sampleDay.ts` — the day this summarises, and `OFFPLAN_TASK_ID`
// - `./scriptDay.ts` — the beats that narrate each section by name

import { useState } from 'react'
import { ProviderIcon } from '@/components/ProviderIcon'
import { OFFPLAN_TASK_ID, sampleDayString, sampleTasks, samplePlanItems } from './sampleDay'
import type { SampleOverview } from './sampleDay'
import { TaskView } from './TutorialSummaryTask'
import type { DayTask } from '@/lib/api-types'

// The re-match picker's ticket list is `demoBoardTickets` (see `./demoWorklog`),
// the same list the detail panel's picker offers - this file used to keep a
// second copy of it, which is how the two demos drifted apart.

/** The standup block, from the demo's `STANDUP_LINES`. The logout-bug line no
 *  longer cites a key: that work is the example day's UNPLANNED strand and
 *  carries no ticket until the user creates one two beats later, so printing a
 *  key here would answer the question the propose flow is about to ask. */
const STANDUP_LINES = [
  'Shipped activity-feed pagination - the feed now loads instantly (MER-475).',
  'Fixed the random-logout bug that slipped in overnight - ticket to file.',
  'Shared standup notes and planned the sprint (MER-501).',
  'Next up: onboarding empty-state polish rolls to tomorrow.',
]

/** The ticket Meridian proposes for the unplanned work, and the key it lands on
 *  once the user creates it. Exported so the beat and the card agree. */
export const PROPOSED_TICKET = {
  key: 'MER-517',
  issueType: 'Bug',
  title: 'Users randomly logged out mid-session',
  description:
    'Sessions were being invalidated early when two token refreshes raced each'
    + ' other, logging people out mid-session. Timing issue fixed; a draft fix is'
    + ' open for review.',
}

/** "1040" → "5:20 PM". */
export function clock(hhmm: string): string {
  const [h, m] = hhmm.split(':').map(Number)
  const period = h >= 12 ? 'PM' : 'AM'
  return `${((h + 11) % 12) + 1}:${String(m).padStart(2, '0')} ${period}`
}

export function dur(min: number): string {
  const h = Math.floor(min / 60)
  return h > 0 ? (min % 60 ? `${h}h ${min % 60}m` : `${h}h`) : `${min}m`
}

/** "DAILY SUMMARY · WED 22 JUL" for the example day. */
function eyebrowDate(): string {
  const [y, m, d] = sampleDayString().split('-').map(Number)
  return new Date(y, m - 1, d)
    .toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short' })
    .toUpperCase()
}

export function TutorialSummaryCard({ sample, provider }: {
  sample: SampleOverview
  /** The tracker the user actually connected, so the post beat names their board
   *  rather than promising Jira to someone on GitHub. `null` on the solo path,
   *  which drops the post flow to "your board" wording. */
  provider: { id: string; name: string } | null
}) {
  const [taskId, setTaskId] = useState<string | null>(null)
  // `propose` is where the flow STARTS, because the row the tour opens is the
  // day's unplanned work: nothing on the plan covered it, so the draft is a
  // brand-new ticket rather than a match. Picking a ticket by hand flips it to
  // `match`, which is the whole point of offering the picker - the proposal is a
  // suggestion the user can overrule, not a decision already taken.
  const [mode, setMode] = useState<'propose' | 'match'>('propose')
  const [target, setTarget] = useState<string | null>(null)
  const [picking, setPicking] = useState(false)
  const [phase, setPhase] = useState<'idle' | 'posting' | 'posted'>('idle')
  const [copied, setCopied] = useState(false)

  const tasks = sampleTasks()
  const task = tasks.find(t => t.id === taskId) ?? null

  return (
    <div className="absolute inset-0 flex items-center justify-center p-6"
      // Literal rgba: `--win-bg` is a gradient, so a `color-mix` against it is
      // invalid and CSS drops the whole declaration - this backdrop was blurring
      // without dimming. Matches the tour overlay's scrims.
      style={{ zIndex: 5, background: 'rgba(20,16,40,0.6)', backdropFilter: 'blur(3px)' }}>
      {/* `relative` so the close button's `absolute` lands on the CARD rather
          than on the backdrop, which is the nearest positioned ancestor
          otherwise - that put the × in the corner of the screen. */}
      <div className="relative w-full flex flex-col rounded-2xl bg-panel mer-pop overflow-hidden"
        style={{
          maxWidth: 940, maxHeight: '100%',
          border: '1px solid var(--t-card-border)', boxShadow: 'var(--mt-modal-shadow)',
        }}>
        {/* Keyed on which view is in the card, so opening a row and coming back
            fades rather than cutting. The two views share nothing visually - the
            whole day, then one strand of it - so a hard swap read as a different
            window appearing in the same frame. */}
        <div key={task ? 'task' : 'home'}
          className="flex-1 min-h-0 overflow-y-auto nice-scroll mer-tour-pane"
          style={{ padding: '26px 28px 28px' }}>
          {task
            ? <TaskView
                task={task} provider={provider} mode={mode} target={target}
                picking={picking} phase={phase}
                onBack={() => { setTaskId(null); setPicking(false) }}
                onPick={(k) => { setTarget(k); setMode('match'); setPicking(false) }}
                onTogglePicker={() => setPicking(p => !p)}
                onPost={() => {
                  if (phase !== 'idle') return
                  setPhase('posting')
                  setTimeout(() => setPhase('posted'), 1300)
                }}
              />
            : <HomeView
                sample={sample} provider={provider} posted={phase === 'posted'} copied={copied}
                onSelect={(id) => setTaskId(id)}
                onCopy={() => { setCopied(true); setTimeout(() => setCopied(false), 2200) }}
              />}
        </div>
      </div>
    </div>
  )
}

// ── The day ──────────────────────────────────────────────────────────────────

function HomeView({ sample, provider, posted, copied, onSelect, onCopy }: {
  sample: SampleOverview
  provider: { id: string; name: string } | null
  posted: boolean
  copied: boolean
  onSelect: (taskId: string) => void
  onCopy: () => void
}) {
  const plan = samplePlanItems()
  const tasks = sampleTasks()
  // Named rather than derived. "Every task with no ticket" would also sweep in
  // the day's incidental strands — a Slack catch-up, a talk playing while a test
  // suite ran — and this list's whole claim is that what is in it deserved a
  // ticket. The example day says which one that is; a filter would only be
  // guessing at it.
  const offPlan = tasks.filter(t => t.id === OFFPLAN_TASK_ID)
  const done = plan.filter(p => p.is_terminal).length
  const pct = plan.length ? Math.round((done / plan.length) * 100) : 0
  const logged = Math.round(sample.engagedSeconds / 60)

  return (
    <>
      <button data-tour="sum-close" aria-label="Close"
        className="absolute right-5 top-5 inline-flex items-center justify-center rounded-full bg-wrap hover:opacity-70"
        style={{ width: 30, height: 30, color: 'var(--t-muted)', cursor: 'pointer' }}>
        <span className="text-[17px] leading-none">×</span>
      </button>

      {/* ── Hero: the day in a sentence, and the numbers behind it ─────────── */}
      <div data-tour="sum-hero" className="flex items-start gap-8">
        <div className="min-w-0 flex-1">
          <p className="mt-label" style={{ color: 'var(--color-state-proposal)' }}>
            DAILY SUMMARY · {eyebrowDate()}
          </p>
          <h2 className="mt-2" style={{
            font: '800 25px var(--font-sans)', letterSpacing: '-0.03em',
            lineHeight: 1.15, color: 'var(--t-title)',
          }}>
            You had a very productive day
          </h2>
          <p className="mt-body mt-3" style={{ color: 'var(--t-muted)', lineHeight: 1.55, maxWidth: '46ch' }}>
            You wrapped <b style={{ color: 'var(--t-title)' }}>{done} of {plan.length}</b> planned tasks.
            An urgent logout bug pulled you off the third - it rolls to tomorrow, already noted.
          </p>
        </div>

        <div data-tour="sum-score" className="shrink-0 flex items-center gap-5">
          <Donut pct={pct} />
          <div className="flex flex-col gap-2">
            <Stat n={`${done} / ${plan.length}`} l="PLANNED DONE" />
            <Stat n={dur(logged)} l="TIME LOGGED" />
            <Stat n="+1" l="URGENT PICKUP" accent />
          </div>
        </div>
      </div>

      {/* ── The two things worth remembering ───────────────────────────────── */}
      <div data-tour="sum-cards" className="grid grid-cols-2 gap-3 mt-6">
        <Insight glyph="↯" tint="var(--color-state-proposal)" title="Handled the unexpected"
          body="A random-logout bug was not on the plan. You caught it and shipped a fix." />
        <Insight glyph="✦" tint="var(--color-state-approved)" title="New learning"
          body="Cursor pagination made the activity feed feel instant - worth reusing." />
      </div>

      {/* ── The plan, ticked off, and the standup it wrote ─────────────────── */}
      <div className="grid gap-3 mt-6" style={{ gridTemplateColumns: '1.15fr 1fr' }}>
        <div data-tour="sum-plan" className="rounded-xl p-3.5"
          style={{ background: 'var(--t-box)', border: '1px solid var(--t-card-border)' }}>
          <p className="mt-label mb-2.5" style={{ color: 'var(--t-faint-2)' }}>TODAY&apos;S PLAN</p>
          <div className="flex flex-col gap-1">
            {plan.map(p => (
              <PlanRow key={p.task_key} title={p.title} done={p.is_terminal}
                ticket={p.task_key} posted={false} provider={provider} />
            ))}
          </div>

          {/* ── The work that was NOT on the plan ────────────────────────────
              A real day has some, and the summary that hides it is the one
              people stop trusting - it is exactly the work they get asked about
              and cannot account for. It is also the only place the second
              worklog outcome can be taught honestly: these strands matched no
              planned ticket, so Meridian drafted a NEW one rather than filing
              them against the nearest wrong match. The tour opens this row. */}
          <div data-tour="sum-offplan" className="mt-3.5 pt-3 border-t" style={{ borderColor: 'var(--t-card-border)' }}>
            <p className="mt-label mb-2" style={{ color: 'var(--t-faint-2)' }}>
              CAME UP TODAY - NOT ON THE PLAN
            </p>
            <div className="flex flex-col gap-1">
              {offPlan.map(t => (
                <OffPlanRow key={t.id} title={t.title} minutes={t.minutes}
                  posted={posted && t.id === OFFPLAN_TASK_ID} provider={provider}
                  tour={t.id === OFFPLAN_TASK_ID ? 'sum-offplan-row' : undefined}
                  onClick={() => onSelect(t.id)} />
              ))}
            </div>
          </div>
        </div>

        <div data-tour="sum-standup" className="rounded-xl p-3.5"
          style={{ background: 'var(--t-box)', border: '1px solid var(--t-card-border)' }}>
          <div className="flex items-center justify-between mb-2.5">
            <p className="mt-label" style={{ color: 'var(--t-faint-2)' }}>STANDUP - READY TO PASTE</p>
            <button data-tour="sum-copy" onClick={onCopy}
              className="rounded-md px-2 py-1 hover:opacity-80"
              style={{
                fontSize: 11, fontWeight: 700, cursor: 'pointer',
                color: 'var(--color-state-proposal)',
                background: 'color-mix(in srgb, var(--color-state-proposal) 12%, transparent)',
                border: '1px solid color-mix(in srgb, var(--color-state-proposal) 30%, transparent)',
              }}>
              {copied ? 'Copied ✓' : 'Copy'}
            </button>
          </div>
          <div className="flex flex-col gap-1.5">
            {STANDUP_LINES.map(l => (
              <p key={l} className="mt-body-sm" style={{ color: 'var(--t-muted)', lineHeight: 1.5 }}>• {l}</p>
            ))}
          </div>
        </div>
      </div>
    </>
  )
}

/** One strand of unplanned work on the summary's home view. Leads with the time
 *  it took rather than a ticket key, because the whole point of the row is that
 *  it does not have one yet. */
function OffPlanRow({ title, minutes, posted, provider, tour, onClick }: {
  title: string
  minutes: number
  posted: boolean
  provider: { id: string; name: string } | null
  tour?: string
  onClick: () => void
}) {
  return (
    <button data-tour={tour} onClick={onClick}
      className="flex items-center gap-2.5 rounded-lg px-2 py-2 text-left hover:opacity-80"
      style={{ background: 'transparent', border: 'none', cursor: 'pointer' }}>
      <span className="inline-flex items-center justify-center shrink-0 rounded-md"
        style={{
          width: 17, height: 17, fontSize: 11, fontWeight: 800,
          color: 'var(--color-state-pending)',
          background: 'color-mix(in srgb, var(--color-state-pending) 16%, transparent)',
        }}>↯</span>
      <span className="min-w-0 flex-1">
        <span className="mt-body-sm block truncate" style={{ color: 'var(--t-title)' }}>{title}</span>
        <span className="block" style={{ fontSize: 11, color: 'var(--t-faint)' }}>
          {posted
            ? `Filed as ${PROPOSED_TICKET.key}`
            : `${dur(minutes)} · no ticket yet - draft ready`}
        </span>
      </span>
      {posted
        ? <span className="shrink-0 inline-flex items-center gap-1" style={{ fontSize: 11, color: 'var(--color-state-approved)' }}>
            {provider && <ProviderIcon provider={provider.id} size={10} />}✓
          </span>
        : <span className="shrink-0" style={{ fontSize: 14, color: 'var(--t-faint)' }}>›</span>}
    </button>
  )
}

// ── Small parts ──────────────────────────────────────────────────────────────

function PlanRow({ title, done, ticket, posted, provider }: {
  title: string
  done: boolean
  ticket: string
  posted: boolean
  provider: { id: string; name: string } | null
}) {
  const sub = posted ? 'Posted' : done ? 'Done · ready to post' : 'Carried over to tomorrow'
  return (
    <div className="flex items-center gap-2.5 rounded-lg px-2 py-2 text-left"
      style={{ background: 'transparent' }}>
      <span className="inline-flex items-center justify-center shrink-0 rounded-md"
        style={{
          width: 17, height: 17, fontSize: 11, fontWeight: 800,
          color: done ? '#fff' : 'transparent',
          background: done ? 'var(--color-state-approved)' : 'transparent',
          border: done ? 'none' : '1.5px solid var(--t-ctrl-border)',
        }}>✓</span>
      <span className="min-w-0 flex-1">
        <span className="mt-body-sm block truncate" style={{
          color: done ? 'var(--t-faint-2)' : 'var(--t-title)',
          textDecoration: done ? 'line-through' : 'none',
        }}>{title}</span>
        <span className="block" style={{ fontSize: 11, color: 'var(--t-faint)' }}>{sub}</span>
      </span>
      {posted
        ? <span className="shrink-0 inline-flex items-center gap-1" style={{ fontSize: 11, color: 'var(--color-state-approved)' }}>
            {provider && <ProviderIcon provider={provider.id} size={10} />}✓
          </span>
        : <span className="shrink-0" style={{ fontSize: 11, color: 'var(--t-faint)' }}>{ticket}</span>}
    </div>
  )
}

function Donut({ pct }: { pct: number }) {
  const R = 30
  const C = 2 * Math.PI * R
  const dash = (pct / 100) * C
  return (
    <div className="relative shrink-0" style={{ width: 76, height: 76 }}>
      <svg width="76" height="76" viewBox="0 0 76 76">
        <circle cx="38" cy="38" r={R} fill="none" stroke="var(--t-box)" strokeWidth="8" />
        <circle cx="38" cy="38" r={R} fill="none" stroke="var(--color-state-proposal)" strokeWidth="8"
          strokeLinecap="round" strokeDasharray={`${dash} ${C - dash}`} transform="rotate(-90 38 38)" />
      </svg>
      <div className="absolute inset-0 flex flex-col items-center justify-center">
        <span style={{ font: '800 16px var(--font-sans)', color: 'var(--t-title)' }}>{pct}%</span>
        <span style={{ fontSize: 9, color: 'var(--t-faint)' }}>of plan</span>
      </div>
    </div>
  )
}

function Stat({ n, l, accent }: { n: string; l: string; accent?: boolean }) {
  return (
    <div className="flex items-baseline gap-2">
      <span style={{
        font: '800 15px var(--font-sans)',
        color: accent ? 'var(--color-state-proposal)' : 'var(--t-title)',
      }}>{n}</span>
      <span style={{ fontSize: 9.5, letterSpacing: '.06em', color: 'var(--t-faint)' }}>{l}</span>
    </div>
  )
}

function Insight({ glyph, tint, title, body }: {
  glyph: string; tint: string; title: string; body: string
}) {
  return (
    <div className="flex items-start gap-3 rounded-xl p-3.5"
      style={{ background: `color-mix(in srgb, ${tint} 8%, transparent)`, border: `1px solid color-mix(in srgb, ${tint} 22%, transparent)` }}>
      <span className="inline-flex items-center justify-center shrink-0 rounded-lg"
        style={{ width: 26, height: 26, background: `color-mix(in srgb, ${tint} 16%, transparent)`, color: tint, fontSize: 13 }}>
        {glyph}
      </span>
      <div className="min-w-0">
        <p className="mt-body-sm" style={{ color: 'var(--t-title)', fontWeight: 700 }}>{title}</p>
        <p className="mt-body-sm mt-1" style={{ color: 'var(--t-muted)', lineHeight: 1.5 }}>{body}</p>
      </div>
    </div>
  )
}
