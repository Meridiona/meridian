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
import { load as loadData, mutate as mutateData } from '@/lib/bridge'
import type { PlanItem, PlanResponse } from '@/lib/api-types'
import { isPending } from './types'
import { TimeByApp, appTotals } from './TimeByApp'
import type { TimelineData } from './useTimelineData'
import type { ActiveModal } from './MeridianTimelineShell'

export function OverviewPanel({ data, onOpen, onOpenTask }: {
  data: TimelineData
  onOpen: (modal: ActiveModal) => void
  onOpenTask: (key: string, title?: string) => void
}) {
  const { today, isSolo, items, counts, cleanupIssueCount, tasks } = data
  const pendingCount = items.filter(isPending).length
  const focus_s = today?.focus_s ?? 0
  const appTops = today ? appTotals(today.sessions) : []
  const appCount = appTops.length
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
        if (url) window.open(url, '_blank', 'noopener')
      }
    }).catch(e => {
      setToggleError(s => ({ ...s, [t.task_key]: e instanceof Error ? e.message : typeof e === 'string' ? e : 'Couldn’t update the tracker' }))
    }).finally(() => {
      setToggling(s => { const n = { ...s }; delete n[t.task_key]; return n })
    })
  }, [])

  const greetingEyebrow = 'Today at a glance'
  const greetingTitle = isSolo ? 'Your day, in progress' : "You're having a solid day"
  const greetingBody = isSolo
    ? `${fmtDur(focus_s)} of focused activity across ${appCount} app${appCount === 1 ? '' : 's'}.`
    : `${fmtDur(loggedSeconds)} logged across ${loggedCount} work log${loggedCount === 1 ? '' : 's'}.`
      + (pendingCount > 0 ? ` ${pendingCount} draft${pendingCount === 1 ? '' : 's'} waiting for your review.` : '')

  return (
    <div className="h-full overflow-y-auto nice-scroll p-6 space-y-7">
      <div>
        <p className="mt-label" style={{ color: 'var(--t-faint)' }}>{greetingEyebrow}</p>
        <p className="mt-greeting text-title mt-1">{greetingTitle}</p>
        <p className="mt-body mt-1.5" style={{ color: 'var(--t-muted)' }}>{greetingBody}</p>
      </div>

      {!isSolo && pendingCount > 0 && (
        <button onClick={() => onOpen('review')}
          className="w-full text-left rounded-xl px-4 py-3.5 flex items-center gap-3 transition-transform active:scale-[.99]"
          style={{ background: 'linear-gradient(120deg, #8B5CF6, #F472B6)', color: '#fff', boxShadow: '0 10px 24px -8px rgba(219,39,119,0.5)' }}>
          <span className="mt-title-lg shrink-0">{pendingCount}</span>
          <span className="flex-1 min-w-0">
            <p className="mt-title">draft{pendingCount === 1 ? '' : 's'} to review</p>
            <p className="mt-body-sm mt-0.5" style={{ opacity: 0.92 }}>Swipe to approve, dismiss or edit</p>
          </span>
          <span className="mt-body-sm px-2.5 py-1 rounded-full shrink-0" style={{ background: 'rgba(255,255,255,0.25)' }}>Review →</span>
        </button>
      )}

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
          <div className="flex items-center justify-between mb-2.5">
            <p className="mt-label" style={{ color: 'var(--t-faint)' }}>Today&apos;s focus</p>
            <button onClick={() => onOpen('plan')} className="mt-body-sm" style={{ color: 'var(--color-state-proposal)', fontWeight: 700 }}>Edit plan</button>
          </div>
          {focusItems.length > 0 ? (
            <div className="space-y-2">
              {focusItems.map(t => {
                const terminal = overrideTerminal[t.task_key] ?? t.is_terminal
                const busy = !!toggling[t.task_key]
                const err = toggleError[t.task_key]
                return (
                  <div key={t.task_key}
                    role="button" tabIndex={0}
                    onClick={() => onOpenTask(t.task_key, t.title)}
                    onKeyDown={e => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); onOpenTask(t.task_key, t.title) } }}
                    className="w-full text-left flex items-center gap-3 rounded-xl px-4 py-3 bg-card cursor-pointer"
                    style={{ border: '1px solid var(--t-card-border)' }}>
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

      <div>
        <p className="mt-label mb-2.5" style={{ color: 'var(--t-faint)' }}>Today</p>
        <div className="grid grid-cols-3 gap-3">
          <Mini label="Logged" value={fmtDur(loggedSeconds)} tint="var(--color-state-approved)" />
          <Mini label="Focus" value={fmtDur(focus_s)} tint="var(--color-state-proposal)" />
          <Mini label="Drafts" value={String(pendingCount)} tint="var(--color-state-pending)" />
        </div>
      </div>

      <div className="rounded-xl p-5 bg-card" style={{ border: '1px solid var(--t-card-border)' }}>
        <div className="flex items-center justify-between mb-2.5">
          <p className="mt-label" style={{ color: 'var(--t-faint)' }}>Time by app</p>
          {appTops[0] && (
            <p className="mt-body-sm" style={{ color: 'var(--t-faint)' }}>most in {appTops[0].app}</p>
          )}
        </div>
        <TimeByApp sessions={today?.sessions ?? []} />
      </div>

      <EntryRow label="Tasks" hint={`${activeTaskCount} active`} onClick={() => onOpen('tasks')} />
    </div>
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

function Mini({ label, value, tint }: { label: string; value: string; tint: string }) {
  return (
    <div className="rounded-xl p-3" style={{ background: `color-mix(in srgb, ${tint} 14%, transparent)` }}>
      <p className="mt-stat" style={{ color: tint }}>{value}</p>
      <p className="mt-label mt-1" style={{ color: 'var(--t-faint)' }}>{label}</p>
    </div>
  )
}
