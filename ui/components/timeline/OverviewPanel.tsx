//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The right panel's default state (no hour selected). Layout mirrors the
// design mock: eyebrow + greeting + summary line, drafts-to-review CTA,
// board-cleanup CTA, a "Today's focus" plan checklist, a "Today" mini-card
// row (Focus / Drafts — Drafts opens the swipeable review dialog), and the
// time-by-app chart (connected users only for the CTAs/plan; solo users get
// the greeting + Today cards + time-by-app). Narrative + metric fields are adapted from
// the retired TodayView's data; the plan checklist reads the same get_plan
// the Daily plan modal uses. The checkbox writes through for real rather than
// being decorative — via set_plan_task_done, which routes a personal task to
// our DB and a board ticket to its tracker's close/reopen; clicking the row
// body still opens the ticket detail.

'use client'

import { useCallback, useEffect, useMemo, useState } from 'react'
import { fmtDur } from '@/components/atoms'
import { load as loadData, mutate as mutateData, openExternal } from '@/lib/bridge'
import { usePlan, refreshPlan } from '@/components/plan/planStore'
import type { PlanItem, CodingAgentsResponse } from '@/lib/api-types'
import { focusSectionVisible, formatDayLabel, isPending } from './types'
import { TimeByApp, appTotals } from './TimeByApp'
import { TimeByCategory, categoryRows } from './TimeByCategory'
import { UpdateCard } from './UpdateCard'
import type { TimelineData } from './useTimelineData'
import type { ActiveModal } from './MeridianTimelineShell'
import type { SettingsSection } from './settings/types'


export function OverviewPanel({ data, onOpen, onOpenTask, onOpenSettings }: {
  data: TimelineData
  onOpen: (modal: ActiveModal) => void
  onOpenTask: (key: string, title?: string, editable?: boolean) => void
  onOpenSettings: (section?: SettingsSection) => void
}) {
  // `cleanupIssueCount` is still on `data` (computed in useTimelineData) for when
  // the Board Cleanup CTA below re-enables; not destructured while it's disabled.
  const { today, isSolo, items, isToday, day } = data
  const dayLabel = isToday ? 'Today' : formatDayLabel(day)
  // "Today's focus" only reads right on today; a past date shows that day's plan
  // under a neutral "Focus" heading (the day itself is already named above).
  const focusLabel = isToday ? "Today's focus" : 'Focus'
  const pendingCount = items.filter(isPending).length
  // Per-tool coding-agent breakdown (Claude Code/Codex/GitHub Copilot/Cursor
  // Agent) — get_coding_agents, polled independently of get_today since it's
  // its own Tauri command. Used to give Time by App real per-tool rows
  // instead of one generic bucket.
  const [codingAgents, setCodingAgents] = useState<CodingAgentsResponse | null>(null)
  useEffect(() => {
    const fetchAgents = () => loadData<CodingAgentsResponse>('/api/coding-agents', 'get_coding_agents', { day }).then(setCodingAgents).catch(() => {})
    fetchAgents()
    const id = setInterval(fetchAgents, 30_000)
    return () => clearInterval(id)
  }, [day])
  // "Focus" is inclusive of ALL engaged time, not just foreground presence —
  // today.engaged_s (= focus_s + autonomous_s, computed server-side in
  // meridian-core/src/readers/today/mod.rs) folds in autonomous agent time
  // (agent runs that happened while you were away) so Focus can never read
  // lower than the autonomous chunk baked into Coding — the two totals stay
  // in a coherent "Focus ≥ what happened today" relationship instead of
  // Focus looking artificially small next to Coding.
  const focus_s = today?.engaged_s ?? 0
  // Total time in coding-agent TOOLS today (Claude Code/Codex/Copilot/Cursor
  // Agent) — a separate overlay stream from `today.sessions`, already unioned
  // server-side to avoid double-counting overlapping agent/foreground time.
  // NOT shown as its own top-level stat — it's folded into Time by
  // category's "Coding" slice (categoryRows) as the one number that tells
  // the coding story. A standalone "Agent time" card alongside a
  // Coding-inclusive-of-agent-time slice put the same number in two places
  // (once as a subset, once inside a bigger total) with no visual link
  // between them, which read as a mismatch even though both were correct.
  const agent_s = today?.agent_s ?? 0
  // Autonomous-time breakdown disabled for now (commented out, not deleted —
  // the underlying today.autonomous_s data is still computed server-side,
  // this just stops surfacing it in the UI while the framing gets rethought).
  // const autonomous_s = today?.autonomous_s ?? 0
  // get_coding_agents is day-scoped now (it takes the same `day` this panel
  // shows), so its totals are folded in on every day. It used to hardcode
  // today's date, which forced this to drop agent rows entirely on a past day
  // rather than inject the wrong day's coding time — a past day simply showed
  // no agent time at all. Both halves now read the same date.
  const appTops = today ? appTotals(today.sessions, codingAgents?.agents ?? []) : []
  const appCount = appTops.length
  const catTops = today ? categoryRows(today.sessions, agent_s) : []
  // Real worklogs only — is_proposed items carry an 'approved'/'posted' state
  // once a user approves them in-app, but the daemon hasn't necessarily swept
  // them into an actual pm_worklogs row (real ticket created + worklog posted)
  // yet, so counting them here would inflate "Logged" for work not yet logged.
  const loggedItems = items.filter(i => !i.is_proposed && (i.state === 'approved' || i.state === 'posted'))
  const loggedCount = loggedItems.length
  const loggedSeconds = loggedItems.reduce((a, i) => a + (i.time_spent_seconds || 0), 0)

  // "Today's focus" — the locked daily plan. The checkbox writes through to the
  // real tracker (close/reopen), so `overrideTerminal` holds the optimistic
  // result of an in-flight/just-applied toggle until the next `get_plan` poll
  // confirms it — avoids a flicker back to the stale state between the write
  // landing and the 30s poll picking it up.
  const [toggling, setToggling] = useState<Record<string, boolean>>({})
  const [overrideTerminal, setOverrideTerminal] = useState<Record<string, boolean>>({})
  const [toggleError, setToggleError] = useState<Record<string, string>>({})
  // Read through the SHARED plan store rather than a private useState: the
  // planner (PlanView) is a sibling overlay that never unmounts this panel, so a
  // confirm/skip/save there has no way to reach local state and used to sit
  // stale here until the next poll. The store publishes every write to all
  // readers at once. It's keyed by calendar day (daily_plan is keyed by
  // plan_date), so viewing a past date shows THAT day's committed focus and
  // correctly ignores edits to today's plan — and a day with nothing fetched
  // reads EMPTY, so a day switch never flashes the previous day's items.
  // Fetched for every user, including solo/no-tracker — the empty-TODAY nudge
  // below is meant to show for them too (PlanView's composer supports a
  // personal, tracker-free task), and gating the fetch on `isSolo` left it
  // permanently null for them, so the nudge never rendered.
  const { data: plan } = usePlan(day)
  useEffect(() => {
    refreshPlan(day)
    const id = setInterval(() => refreshPlan(day), 30_000)
    return () => clearInterval(id)
  }, [day])
  const focusItems = useMemo(() => (plan?.confirmed ? plan.plan : []), [plan])

  // Toggle a focus item's done state. This goes through `set_plan_task_done`,
  // NOT `apply_ticket_fix` — the latter only knows real trackers, so ticking a
  // personal (provider 'local') task died with `provider "local" is not
  // configured`. The command branches on who owns the task: a personal one is a
  // direct DB write, a board ticket still gets the tracker's close/reopen. Some
  // providers (Trello, Azure DevOps) have no reliable done/not-done mapping and
  // redirect to the ticket in the browser instead of writing in-app; only a true
  // `applied` result flips the checkbox.
  const toggleDone = useCallback((t: PlanItem, currentlyTerminal: boolean) => {
    setToggling(s => ({ ...s, [t.task_key]: true }))
    setToggleError(s => { if (!(t.task_key in s)) return s; const n = { ...s }; delete n[t.task_key]; return n })
    mutateData<{ status: string; browse_url?: string; reason?: string }>(
      '/api/plan/task-done', 'set_plan_task_done', { task_key: t.task_key, done: !currentlyTerminal },
    ).then(data => {
      if (data.status === 'applied') {
        setOverrideTerminal(s => ({ ...s, [t.task_key]: !currentlyTerminal }))
        refreshPlan(day)
      } else {
        const url = data.browse_url || t.url
        if (url) openExternal(url)
      }
    }).catch(e => {
      setToggleError(s => ({ ...s, [t.task_key]: e instanceof Error ? e.message : typeof e === 'string' ? e : 'Couldn’t update the task' }))
    }).finally(() => {
      setToggling(s => { const n = { ...s }; delete n[t.task_key]; return n })
    })
  }, [day])

  const greetingEyebrow = isToday ? 'Today at a glance' : `${dayLabel} at a glance`
  const greetingTitle = isSolo ? 'Your day, in progress' : "You're having a solid day"
  const greetingBody = isSolo
    ? `${fmtDur(focus_s)} of focused activity across ${appCount} app${appCount === 1 ? '' : 's'}.`
    : `${fmtDur(loggedSeconds)} logged across ${loggedCount} work log${loggedCount === 1 ? '' : 's'}.`
      + (pendingCount > 0 ? ` ${pendingCount} draft${pendingCount === 1 ? '' : 's'} waiting for your review.` : '')

  return (
    <div className="h-full overflow-y-auto nice-scroll p-6 space-y-7">
      {/* DMG update CTA — only renders when a newer version is available; sits
          at the top of the sidebar so it's noticeable without stealing the
          whole header. Sibling of the tray popover's update banner. */}
      <UpdateCard />

      <div>
        <p className="mt-label" style={{ color: 'var(--t-faint)' }}>{greetingEyebrow}</p>
        <p className="mt-greeting text-title mt-1">{greetingTitle}</p>
        <p className="mt-body mt-1.5" style={{ color: 'var(--t-muted)' }}>{greetingBody}</p>
      </div>

      <div className="rounded-xl overflow-hidden bg-card p-4" style={{ border: '1px solid var(--t-card-border)' }}>
        <div className="mb-3"><SectionHeading>{dayLabel}</SectionHeading></div>
        {/* Solo mode: Drafts requires PM-matched worklogs, which don't exist
            without a tracker — showing it here would always read some stale
            count. Focus is the one stat that's real. */}
        {isSolo ? (
          <div className="grid grid-cols-1 gap-3">
            <Mini label="Focus" value={fmtDur(focus_s)} />
          </div>
        ) : (
          // Just Focus + Drafts — Drafts doubles as the entry point into the
          // swipeable review dialog (same one FloatingDraftsPill opens), so
          // it renders as a button once there's something to review, with an
          // accent color to signal it's live/actionable rather than a plain
          // counter like the other tiles used to be.
          <div className="grid grid-cols-2 gap-3">
            <Mini label="Focus" value={fmtDur(focus_s)} />
            <Mini label="Drafts" value={String(pendingCount)}
              onClick={pendingCount > 0 ? () => onOpen('review') : undefined}
              accent={pendingCount > 0} />
          </div>
        )}
      </div>

      {/* Board Cleanup CTA — temporarily disabled (not deleted); re-enable by
          uncommenting this block. This was the only UI entry point into the
          Cleanup flow (see also MeridianTimelineShell.tsx's `'cleanup'`
          modal-render branch, disabled alongside it).
      {!isSolo && cleanupIssueCount > 0 && (
        <button onClick={() => onOpen('cleanup')}
          className="w-full text-left rounded-xl px-4 py-3 flex items-center gap-2.5"
          style={{ background: 'color-mix(in srgb, var(--color-state-pending) 12%, transparent)', border: '1px solid color-mix(in srgb, var(--color-state-pending) 30%, transparent)' }}>
          <span className="inline-flex items-center justify-center rounded-full shrink-0 text-[13px]"
            style={{ width: 26, height: 26, background: 'color-mix(in srgb, var(--color-state-pending) 20%, transparent)' }}>🧹</span>
          <span className="flex-1 min-w-0">
            <p className="mt-card-title" style={{ color: 'var(--color-state-pending)' }}>Board cleanup available</p>
            <p className="mt-body-sm mt-0.5" style={{ color: 'var(--t-muted)' }}>{cleanupIssueCount} issue{cleanupIssueCount === 1 ? '' : 's'} make matching harder</p>
          </span>
          <span style={{ color: 'var(--color-state-pending)' }}>→</span>
        </button>
      )}
      */}

      {/* Editing is a today-only action — you plan the day you're in, not the
          past. On a past date the section is read-only: it shows that day's
          committed focus (or a quiet empty note), with no Edit/Add
          affordances. `focusLabel` relabels the heading off "Today's".
          Shown to every user, solo/no-tracker included — PlanView's composer
          supports a personal, tracker-free task (see its `boardEmpty` path),
          and `toggleDone` below already routes a personal task to our own DB
          rather than a tracker, so nothing here needs a board. This used to
          gate everything except the empty-today nudge on `!isSolo`, which meant
          a solo user was invited to plan and then never shown the plan they
          committed. See `focusSectionVisible` for the whole rule. */}
      {focusSectionVisible({ isToday, planLoaded: !!plan, itemCount: focusItems.length }) && (
        <div>
          {focusItems.length === 0 && isToday ? (
            <div>
              <div className="mb-2.5"><SectionHeading>Today&apos;s focus</SectionHeading></div>
              <EmptyPlanNudge onOpen={() => onOpen('plan')} />
            </div>
          ) : focusItems.length > 0 ? (
            <div className="rounded-xl overflow-hidden bg-card" style={{ border: '1px solid var(--t-card-border)' }}>
              <div className="flex items-center justify-between px-4 py-3">
                <SectionHeading>{focusLabel}</SectionHeading>
                {isToday && <button onClick={() => onOpen('plan')} className="mt-body-sm" style={{ color: 'var(--t-accent)', fontWeight: 700 }}>Edit plan</button>}
              </div>
              {(() => {
                const doneCount = focusItems.filter(t => overrideTerminal[t.task_key] ?? t.is_terminal).length
                return (
                  <div className="flex items-center gap-2 px-4 py-3">
                    <span className="flex-1 h-1 rounded-full overflow-hidden bg-track">
                      <span className="block h-full rounded-full transition-all"
                        style={{ width: `${(doneCount / focusItems.length) * 100}%`, background: 'var(--t-accent)' }} />
                    </span>
                    <span className="mt-mono-sm text-[10.5px] shrink-0" style={{ color: 'var(--t-faint)' }}>{Math.round((doneCount / focusItems.length) * 100)}%</span>
                  </div>
                )
              })()}
              {focusItems.map(t => {
                const terminal = overrideTerminal[t.task_key] ?? t.is_terminal
                const busy = !!toggling[t.task_key]
                const err = toggleError[t.task_key]
                return (
                  <div key={t.task_key}
                    role="button" tabIndex={0}
                    onClick={() => onOpenTask(t.task_key, t.title, isToday)}
                    onKeyDown={e => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); onOpenTask(t.task_key, t.title, isToday) } }}
                    className="w-full text-left flex items-center gap-3 px-4 py-3 cursor-pointer">
                    <button
                      onClick={e => { e.stopPropagation(); toggleDone(t, terminal) }}
                      disabled={busy}
                      aria-label={terminal ? `Reopen ${t.task_key}` : `Mark ${t.task_key} done`}
                      className="inline-flex items-center justify-center rounded-md shrink-0 transition-opacity"
                      style={{
                        width: 18, height: 18,
                        background: terminal ? 'var(--btn-primary-bg)' : 'transparent',
                        border: terminal ? 'none' : '1.5px solid var(--t-hair)',
                        opacity: busy ? 0.5 : 1,
                      }}>
                      {!busy && terminal && <span style={{ color: '#fff', fontSize: 11, lineHeight: 1 }}>✓</span>}
                    </button>
                    <span className="flex-1 min-w-0">
                      <span className={`mt-body block truncate ${terminal ? 'line-through' : ''}`}
                        style={{ color: terminal ? 'var(--t-faint)' : 'var(--t-title)' }}>{t.title}</span>
                      {err && <span className="mt-body-sm block truncate" style={{ color: 'var(--color-state-pending)' }}>{err}</span>}
                    </span>
                    <span className="mt-mono-sm text-[11px] px-1.5 py-0.5 rounded bg-key-bg text-key-text shrink-0">{t.task_key}</span>
                  </div>
                )
              })}
            </div>
          ) : (
            // Reachable only for a past day (the outer condition excludes
            // today when empty) — a quiet historical fact, no CTA.
            <div>
              <div className="mb-2.5"><SectionHeading>{focusLabel}</SectionHeading></div>
              <div className="rounded-xl px-4 py-3.5" style={{ border: '1px dashed var(--t-hair)' }}>
                <p className="mt-body-sm" style={{ color: 'var(--t-muted)' }}>No focus was planned for {dayLabel}.</p>
              </div>
            </div>
          )}
        </div>
      )}

      <div className="rounded-xl p-5 bg-card" style={{ border: '1px solid var(--t-card-border)' }}>
        <div className="flex items-center justify-between mb-2.5">
          <SectionHeading>Time by app</SectionHeading>
          {appTops[0] && (
            <p className="mt-body-sm" style={{ color: 'var(--t-faint)' }}>most in {appTops[0].app}</p>
          )}
        </div>
        <TimeByApp sessions={today?.sessions ?? []} agentTotals={isToday ? (codingAgents?.agents ?? []) : []} />
      </div>

      <div className="rounded-xl p-5 bg-card" style={{ border: '1px solid var(--t-card-border)' }}>
        <div className="flex items-center justify-between mb-2.5">
          <SectionHeading>Time by category</SectionHeading>
          {catTops[0] && (
            <p className="mt-body-sm" style={{ color: 'var(--t-faint)' }}>most in {catTops[0].label}</p>
          )}
        </div>
        <TimeByCategory sessions={today?.sessions ?? []} agentSeconds={agent_s} />
      </div>

      {isSolo && (
        <button onClick={() => onOpenSettings('integrations')}
          className="w-full text-left rounded-xl px-4 py-3 flex items-center gap-2.5 mt-card-hover"
          style={{ background: 'color-mix(in srgb, var(--t-accent) 12%, transparent)', border: '1px solid color-mix(in srgb, var(--t-accent) 30%, transparent)' }}>
          <span className="inline-flex items-center justify-center rounded-full shrink-0 text-[13px]"
            style={{ width: 26, height: 26, background: 'color-mix(in srgb, var(--t-accent) 20%, transparent)' }}>🔗</span>
          <span className="flex-1 min-w-0">
            <p className="mt-card-title" style={{ color: 'var(--t-accent)' }}>Auto-post your work logs</p>
            <p className="mt-body-sm mt-0.5" style={{ color: 'var(--t-muted)' }}>Connect a tracker to match today&apos;s activity automatically</p>
          </span>
          <span style={{ color: 'var(--t-accent)' }}>→</span>
        </button>
      )}
    </div>
  )
}

// Bolder than the default faint-uppercase `.mt-label` eyebrow — used for this
// panel's actual section headings (Today / Today's focus / Time by app /
// Time by category) so they read as real section breaks, not micro-labels.
function SectionHeading({ children }: { children: React.ReactNode }) {
  return (
    <p style={{ font: "800 13px var(--font-sans)", color: 'var(--t-title)' }}>
      {children}
    </p>
  )
}

// The plan feature's empty-state CTA, promoted to the top of the "Today's focus"
// section so it's the first thing a user sees there rather than a plain dashed
// box at the bottom. Soft/pastel (color-mix over the panel surface, same recipe
// as the cleanup/tracker-connect CTAs elsewhere in this file) so it reads as an
// inviting nudge, not a warning — purple (--color-state-proposal) matches the
// rest of the plan feature's own color language (the "Edit plan"/progress-bar/
// done-check accents), so this reads as the same feature rather than a new,
// unrelated color.
function EmptyPlanNudge({ onOpen }: { onOpen: () => void }) {
  return (
    <button onClick={onOpen}
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
  )
}

// `onClick` + `accent` turn this into a live entry point (used for Drafts,
// once there's something to review) rather than a plain counter — accent
// colors the value the same amber as FloatingDraftsPill so the two read as
// the same affordance, and the button semantics + hover/press feedback
// signal it's clickable without adding a whole separate visual style.
function Mini({ label, value, sub, onClick, accent }: {
  label: string; value: string; sub?: string; onClick?: () => void; accent?: boolean
}) {
  const content = (
    <>
      <p className="mt-stat whitespace-nowrap"
        style={{ color: accent ? 'var(--color-state-pending)' : 'var(--t-title)' }}>{value}</p>
      <p className="mt-label mt-1" style={{ color: 'var(--t-faint)' }}>{label}</p>
      {sub && <p className="mt-body-sm mt-0.5" style={{ color: 'var(--t-faint)', fontSize: 10.5 }}>{sub}</p>}
    </>
  )
  if (onClick) {
    return (
      <button onClick={onClick}
        className="w-full text-left rounded-xl p-3 bg-box mt-card-hover transition-transform active:scale-95 cursor-pointer">
        {content}
      </button>
    )
  }
  return <div className="rounded-xl p-3 bg-box">{content}</div>
}
