//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The walkthrough's own right panel.
//
// Visually a twin of `OverviewPanel`, but a SEPARATE component on purpose. The
// first version of this threaded a `sample` prop through OverviewPanel,
// RightPanel and the shell, which put walkthrough branches inside the panel the
// real product renders every second of every day — five extra conditionals in a
// component whose job has nothing to do with onboarding, and a standing risk of
// a demo value leaking into a real screen. The walkthrough is a temporary,
// self-contained surface; it should own its own pixels.
//
// What IS shared is the two charts (`TimeByApp` / `TimeByCategory`), because
// they are pure presentational components over a `sessions` array with no
// product logic in them. Sharing those is reuse; sharing the panel was coupling.
//
// # Who calls this
// [`TutorialScreen`] — the only consumer.
//
// # Related
// - `./sampleDay.ts` — every number rendered here
// - `components/timeline/OverviewPanel.tsx` — the real panel this mirrors

import { fmtDur } from '@/components/atoms'
import { TimeByApp } from '@/components/timeline/TimeByApp'
import { TimeByCategory } from '@/components/timeline/TimeByCategory'
import type { SampleOverview } from './sampleDay'

/** Marks the control a beat points at when it asks the user to set a daily
 *  plan. Lives on whichever plan affordance is currently rendered. */
export const PLAN_TOUR_ATTR = 'plan-open'

export function TutorialPanel({ sample, phase, onOpenPlan }: {
  sample: SampleOverview
  /** `empty` — the walkthrough's first half, before anything is demonstrated:
   *  zeroed stats and the plan nudge, matching the blank timeline beside it.
   *  `example` — the second half, showing the fully-populated example day. */
  phase: 'empty' | 'example'
  onOpenPlan: () => void
}) {
  const isExample = phase === 'example'

  return (
    <div className="h-full overflow-y-auto nice-scroll p-6 space-y-7">
      <div>
        <p className="mt-label" style={{ color: 'var(--t-faint)' }}>Today at a glance</p>
        <p className="mt-greeting text-title mt-1">
          {isExample ? "You're having a solid day" : 'Your day, just started'}
        </p>
        <p className="mt-body mt-1.5" style={{ color: 'var(--t-muted)' }}>
          {isExample
            ? sample.greetingBody
            : 'Nothing tracked yet. Meridian starts filling this in as you work.'}
        </p>
      </div>

      <div className="rounded-xl overflow-hidden bg-card p-4" style={{ border: '1px solid var(--t-card-border)' }}>
        <div className="mb-3"><SectionHeading>Today</SectionHeading></div>
        <div className="grid grid-cols-2 gap-3">
          <Mini label="Focus" value={isExample ? fmtDur(sample.engagedSeconds) : '0m'} />
          <Mini label="Drafts" value={isExample ? String(sample.pendingCount) : '0'}
            accent={isExample && sample.pendingCount > 0} />
        </div>
      </div>

      <div>
        {isExample ? (
          <div className="rounded-xl overflow-hidden bg-card" style={{ border: '1px solid var(--t-card-border)' }}>
            <div className="flex items-center justify-between px-4 py-3">
              <SectionHeading>Today&apos;s focus</SectionHeading>
            </div>
            <FocusProgress items={sample.focusItems} />
            {sample.focusItems.map(t => (
              <div key={t.task_key} className="w-full text-left flex items-center gap-3 px-4 py-3">
                <span className="inline-flex items-center justify-center rounded-md shrink-0"
                  style={{
                    width: 18, height: 18,
                    background: t.is_terminal ? 'var(--btn-primary-bg)' : 'transparent',
                    border: t.is_terminal ? 'none' : '1.5px solid var(--t-hair)',
                  }}>
                  {t.is_terminal && <span style={{ color: '#fff', fontSize: 11, lineHeight: 1 }}>✓</span>}
                </span>
                <span className="flex-1 min-w-0">
                  <span className={`mt-body block truncate ${t.is_terminal ? 'line-through' : ''}`}
                    style={{ color: t.is_terminal ? 'var(--t-faint)' : 'var(--t-title)' }}>{t.title}</span>
                </span>
                <span className="mt-mono-sm text-[11px] px-1.5 py-0.5 rounded bg-key-bg text-key-text shrink-0">{t.task_key}</span>
              </div>
            ))}
          </div>
        ) : (
          <>
            <div className="mb-2.5"><SectionHeading>Today&apos;s focus</SectionHeading></div>
            {/* The one live control on this surface: it opens the REAL planner,
                because the walkthrough is genuinely collecting the user's plan
                here rather than miming it. */}
            <button onClick={onOpenPlan}
              data-tour={PLAN_TOUR_ATTR}
              className="w-full text-left rounded-xl px-4 py-3.5 flex items-center gap-2.5 mt-card-hover"
              style={{
                background: 'color-mix(in srgb, var(--t-accent) 12%, transparent)',
                border: '1px solid color-mix(in srgb, var(--t-accent) 30%, transparent)',
              }}>
              <span className="inline-flex items-center justify-center rounded-full shrink-0 text-[13px]"
                style={{ width: 26, height: 26, background: 'color-mix(in srgb, var(--t-accent) 20%, transparent)' }}>✨</span>
              <span className="flex-1 min-w-0">
                <p className="mt-card-title" style={{ color: 'var(--t-accent)' }}>What are you working on today?</p>
                <p className="mt-body-sm mt-0.5" style={{ color: 'var(--t-muted)' }}>Add a few tasks so Meridian can help you stay on track</p>
              </span>
              <span style={{ color: 'var(--t-accent)' }}>→</span>
            </button>
          </>
        )}
      </div>

      <div className="rounded-xl p-5 bg-card" style={{ border: '1px solid var(--t-card-border)' }}>
        <div className="flex items-center justify-between mb-2.5">
          <SectionHeading>Time by app</SectionHeading>
          {isExample && <p className="mt-body-sm" style={{ color: 'var(--t-faint)' }}>most in Claude Code</p>}
        </div>
        <TimeByApp sessions={isExample ? sample.appSessions : []} />
      </div>

      <div className="rounded-xl p-5 bg-card" style={{ border: '1px solid var(--t-card-border)' }}>
        <div className="mb-2.5"><SectionHeading>Time by category</SectionHeading></div>
        <TimeByCategory sessions={isExample ? sample.catSessions : []} />
      </div>
    </div>
  )
}

function FocusProgress({ items }: { items: SampleOverview['focusItems'] }) {
  const done = items.filter(t => t.is_terminal).length
  const pct = items.length ? Math.round((done / items.length) * 100) : 0
  return (
    <div className="flex items-center gap-2 px-4 py-3">
      <span className="flex-1 h-1 rounded-full overflow-hidden bg-track">
        <span className="block h-full rounded-full" style={{ width: `${pct}%`, background: 'var(--t-accent)' }} />
      </span>
      <span className="mt-mono-sm text-[10.5px] shrink-0" style={{ color: 'var(--t-faint)' }}>{pct}%</span>
    </div>
  )
}

function SectionHeading({ children }: { children: React.ReactNode }) {
  return <p style={{ font: '800 13px var(--font-sans)', color: 'var(--t-title)' }}>{children}</p>
}

function Mini({ label, value, accent }: { label: string; value: string; accent?: boolean }) {
  return (
    <div className="rounded-xl p-3 bg-box">
      <p className="mt-stat whitespace-nowrap"
        style={{ color: accent ? 'var(--color-state-pending)' : 'var(--t-title)' }}>{value}</p>
      <p className="mt-label mt-1" style={{ color: 'var(--t-faint)' }}>{label}</p>
    </div>
  )
}
