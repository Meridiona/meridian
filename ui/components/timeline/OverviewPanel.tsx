//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The right panel's default state (no hour selected). Layout mirrors the
// design mock: eyebrow + greeting + summary line, drafts-to-review CTA,
// board-cleanup CTA, a "Today's focus" plan checklist, a "Today" mini-card
// row (Logged / Focus / Drafts), the time-by-app chart, and a Tasks entry
// point (connected users only for the CTAs/plan; solo users get the greeting
// + Today cards + time-by-app). Narrative + metric fields are adapted from
// the retired TodayView's data; the plan checklist reads the same get_plan
// the Daily plan modal uses. The checkbox writes through to the real tracker
// (apply_ticket_fix close/reopen — same write-back path as board hygiene)
// instead of being purely decorative; clicking the row body still opens the
// ticket detail.

'use client'

import { useCallback, useEffect, useMemo, useState } from 'react'
import { fmtDur } from '@/components/atoms'
import { load as loadData, mutate as mutateData, openExternal } from '@/lib/bridge'
import type { PlanItem, PlanResponse, CodingAgentsResponse } from '@/lib/api-types'
import { formatDayLabel, isPending } from './types'
import { TimeByApp, appTotals } from './TimeByApp'
import { TimeByCategory, categoryRows } from './TimeByCategory'
import { UpdateCard } from './UpdateCard'
import type { TimelineData } from './useTimelineData'
import type { ActiveModal } from './MeridianTimelineShell'
import type { SettingsSection } from './settings/types'


export function OverviewPanel({ data, onOpen, onOpenTask, onOpenSettings }: {
  data: TimelineData
  onOpen: (modal: ActiveModal) => void
  onOpenTask: (key: string, title?: string) => void
  onOpenSettings: (section?: SettingsSection) => void
}) {
  const { today, isSolo, items, cleanupIssueCount, tasks, isToday, day } = data
  const dayLabel = isToday ? 'Today' : formatDayLabel(day)
  const pendingCount = items.filter(isPending).length
  // Per-tool coding-agent breakdown (Claude Code/Codex/GitHub Copilot/Cursor
  // Agent) — get_coding_agents, polled independently of get_today since it's
  // its own Tauri command. Used to give Time by App real per-tool rows
  // instead of one generic bucket.
  const [codingAgents, setCodingAgents] = useState<CodingAgentsResponse | null>(null)
  useEffect(() => {
    const fetchAgents = () => loadData<CodingAgentsResponse>('/api/coding-agents', 'get_coding_agents').then(setCodingAgents).catch(() => {})
    fetchAgents()
    const id = setInterval(fetchAgents, 30_000)
    return () => clearInterval(id)
  }, [])
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
  // get_coding_agents has no date param (it always reads today's totals —
  // tray/src-tauri/src/commands/dashboard.rs hardcodes today_string()), so
  // only fold it in while viewing today; on a past day it would inject
  // TODAY's coding time into that day's app breakdown.
  const appTops = today ? appTotals(today.sessions, isToday ? (codingAgents?.agents ?? []) : []) : []
  const appCount = appTops.length
  const catTops = today ? categoryRows(today.sessions, agent_s) : []
  // Pull the "Coding" number straight out of the SAME merged rows the
  // category chart renders, rather than recomputing it — guarantees the top
  // stat and the pie slice can never disagree, since they're reading the
  // identical value, not two independently-derived numbers that happen to
  // match today and drift apart the next time either side changes.
  const codingRow = catTops.find(r => r.cat === 'coding')
  const coding_s = codingRow?.seconds ?? 0
  // const codingSub = agent_s > 0 && autonomous_s > 0 ? `incl. ${fmtDur(autonomous_s)} autonomous` : undefined
  // Real worklogs only — is_proposed items carry an 'approved'/'posted' state
  // once a user approves them in-app, but the daemon hasn't necessarily swept
  // them into an actual pm_worklogs row (real ticket created + worklog posted)
  // yet, so counting them here would inflate "Logged" for work not yet logged.
  const loggedItems = items.filter(i => !i.is_proposed && (i.state === 'approved' || i.state === 'posted'))
  const loggedCount = loggedItems.length
  const loggedSeconds = loggedItems.reduce((a, i) => a + (i.time_spent_seconds || 0), 0)
  const activeTaskCount = tasks.filter(t => !t.is_terminal).length

  // "Today's focus" — the locked daily plan. The checkbox writes through to the
  // real tracker (close/reopen), so `overrideTerminal` holds the optimistic
  // result of an in-flight/just-applied toggle until the next `get_plan` poll
  // confirms it — avoids a flicker back to the stale state between the write
  // landing and the 30s poll picking it up.
  const [plan, setPlan] = useState<PlanResponse | null>(null)
  const [toggling, setToggling] = useState<Record<string, boolean>>({})
  const [overrideTerminal, setOverrideTerminal] = useState<Record<string, boolean>>({})
  const [toggleError, setToggleError] = useState<Record<string, string>>({})
  useEffect(() => {
    if (isSolo) return
    const fetchPlan = () => loadData<PlanResponse>('/api/plan', 'get_plan').then(setPlan).catch(() => {})
    fetchPlan()
    const id = setInterval(fetchPlan, 30_000)
    return () => clearInterval(id)
  }, [isSolo])
  const focusItems = useMemo(() => (plan?.confirmed ? plan.plan : []), [plan])

  // Toggle a focus item's done state on its real tracker. `close`/`reopen` are
  // the same apply_ticket_fix path board hygiene uses — some providers (Trello,
  // Azure DevOps) have no reliable done/not-done mapping and redirect to the
  // ticket in the browser instead of writing in-app; only a true `applied`
  // result flips the checkbox.
  const toggleDone = useCallback((t: PlanItem, currentlyTerminal: boolean) => {
    const field = currentlyTerminal ? 'reopen' : 'close'
    setToggling(s => ({ ...s, [t.task_key]: true }))
    setToggleError(s => { if (!(t.task_key in s)) return s; const n = { ...s }; delete n[t.task_key]; return n })
    mutateData<{ result: { status: string; browse_url?: string; reason?: string } }>(
      '/api/triage/apply', 'apply_ticket_fix', { provider: t.provider, key: t.task_key, field, value: '' },
    ).then(data => {
      if (data.result.status === 'applied') {
        setOverrideTerminal(s => ({ ...s, [t.task_key]: !currentlyTerminal }))
        loadData<PlanResponse>('/api/plan', 'get_plan').then(setPlan).catch(() => {})
      } else {
        const url = data.result.browse_url || t.url
        if (url) openExternal(url)
      }
    }).catch(e => {
      setToggleError(s => ({ ...s, [t.task_key]: e instanceof Error ? e.message : typeof e === 'string' ? e : 'Couldn’t update the tracker' }))
    }).finally(() => {
      setToggling(s => { const n = { ...s }; delete n[t.task_key]; return n })
    })
  }, [])

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
        {/* Solo mode: Logged/Drafts both require PM-matched worklogs, which
            don't exist without a tracker — showing them here would always
            read "0s"/some stale count. Focus is the one stat that's real.
            One uniform card style for every tile (no per-stat hue) — these
            are plain counters, not distinct categories that need color
            coding to tell apart. */}
        {isSolo ? (
          <div className={`grid gap-3 ${coding_s > 0 ? 'grid-cols-2' : 'grid-cols-1'}`}>
            <Mini label="Focus" value={fmtDur(focus_s)} />
            {coding_s > 0 && <Mini label="Coding" value={fmtDur(coding_s)} />}
          </div>
        ) : (
          // 2×2 (not 4-across) — a 4-column grid in a ~340px panel left no
          // room for "3h 24m"-length values without wrapping. Order: Focus
          // leads, Coding next when there's agent/coding time, then the
          // remaining two.
          <div className="grid grid-cols-2 gap-3">
            <Mini label="Focus" value={fmtDur(focus_s)} />
            {coding_s > 0 && <Mini label="Coding" value={fmtDur(coding_s)} />}
            <Mini label="Logged" value={fmtDur(loggedSeconds)} />
            <Mini label="Drafts" value={String(pendingCount)} />
          </div>
        )}
      </div>

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

      {!isSolo && (
        <div>
          {focusItems.length === 0 && (
            <div className="flex items-center justify-between mb-2.5">
              <SectionHeading>Today&apos;s focus</SectionHeading>
              <button onClick={() => onOpen('plan')} className="mt-body-sm" style={{ color: 'var(--color-state-proposal)', fontWeight: 700 }}>Edit plan</button>
            </div>
          )}
          {focusItems.length > 0 ? (
            <div className="rounded-xl overflow-hidden bg-card" style={{ border: '1px solid var(--t-card-border)' }}>
              <div className="flex items-center justify-between px-4 py-3">
                <SectionHeading>Today&apos;s focus</SectionHeading>
                <button onClick={() => onOpen('plan')} className="mt-body-sm" style={{ color: 'var(--color-state-proposal)', fontWeight: 700 }}>Edit plan</button>
              </div>
              {(() => {
                const doneCount = focusItems.filter(t => overrideTerminal[t.task_key] ?? t.is_terminal).length
                return (
                  <div className="flex items-center gap-2 px-4 py-3">
                    <span className="flex-1 h-1 rounded-full overflow-hidden bg-track">
                      <span className="block h-full rounded-full transition-all"
                        style={{ width: `${(doneCount / focusItems.length) * 100}%`, background: 'var(--color-state-proposal)' }} />
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
                    onClick={() => onOpenTask(t.task_key, t.title)}
                    onKeyDown={e => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); onOpenTask(t.task_key, t.title) } }}
                    className="w-full text-left flex items-center gap-3 px-4 py-3 cursor-pointer">
                    <button
                      onClick={e => { e.stopPropagation(); toggleDone(t, terminal) }}
                      disabled={busy}
                      aria-label={terminal ? `Reopen ${t.task_key}` : `Mark ${t.task_key} done`}
                      className="inline-flex items-center justify-center rounded-md shrink-0 transition-opacity"
                      style={{
                        width: 18, height: 18,
                        background: terminal ? 'var(--color-state-proposal)' : 'transparent',
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
            <button onClick={() => onOpen('plan')}
              className="w-full text-left rounded-xl px-4 py-3.5" style={{ border: '1px dashed var(--t-hair)' }}>
              <p className="mt-body-sm" style={{ color: 'var(--t-muted)' }}>Nothing planned yet for today.</p>
              <p className="mt-body-sm mt-0.5" style={{ color: 'var(--color-state-proposal)', fontWeight: 700 }}>Add tasks to your plan →</p>
            </button>
          )}
        </div>
      )}

      {/* Tasks entry pulls from the connected tracker — with no PM
          integration there's nothing to pull, so it would always read
          "0 active". Connected users only. */}
      {!isSolo && (
        <EntryRow label="Tasks" hint={`${activeTaskCount} active`} onClick={() => onOpen('tasks')} />
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
          style={{ background: 'color-mix(in srgb, var(--color-state-proposal) 12%, transparent)', border: '1px solid color-mix(in srgb, var(--color-state-proposal) 30%, transparent)' }}>
          <span className="inline-flex items-center justify-center rounded-full shrink-0 text-[13px]"
            style={{ width: 26, height: 26, background: 'color-mix(in srgb, var(--color-state-proposal) 20%, transparent)' }}>🔗</span>
          <span className="flex-1 min-w-0">
            <p className="mt-card-title" style={{ color: 'var(--color-state-proposal)' }}>Auto-post your work logs</p>
            <p className="mt-body-sm mt-0.5" style={{ color: 'var(--t-muted)' }}>Connect a tracker to match today&apos;s activity automatically</p>
          </span>
          <span style={{ color: 'var(--color-state-proposal)' }}>→</span>
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

function EntryRow({ label, hint, onClick }: { label: string; hint: string; onClick: () => void }) {
  return (
    <button onClick={onClick}
      className="w-full flex items-center gap-3 rounded-xl px-5 py-3.5 bg-card"
      style={{ border: '1px solid var(--t-card-border)' }}>
      <span className="mt-title text-title flex-1 text-left">{label}</span>
      <span className="mt-mono-sm text-[11px]" style={{ color: 'var(--t-faint)' }}>{hint}</span>
      <span style={{ color: 'var(--t-faint)' }}>›</span>
    </button>
  )
}

function Mini({ label, value, sub }: { label: string; value: string; sub?: string }) {
  return (
    <div className="rounded-xl p-3 bg-box">
      <p className="mt-stat text-title whitespace-nowrap">{value}</p>
      <p className="mt-label mt-1" style={{ color: 'var(--t-faint)' }}>{label}</p>
      {sub && <p className="mt-body-sm mt-0.5" style={{ color: 'var(--t-faint)', fontSize: 10.5 }}>{sub}</p>}
    </div>
  )
}
