//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

import { useCallback, useEffect, useMemo, useState } from 'react'
import { TaskKey, ProviderGlyph, Card, SectionHead } from '@/components/atoms'
import HygieneDialog from '@/components/HygieneDialog'
import { hasMustFix, type HygieneIssue } from '@/lib/hygiene'
import type { TaskSummary, TasksResponse } from '@/app/api/tasks/route'

// Severity → dot + tone. Warm editorial palette: warn (amber) is the alert.
const TONE = {
  must_fix: 'var(--warn)',
  optional: 'var(--accent)',
  review: 'var(--ink-4)',
} as const

export default function CleanupView() {
  const [tasks, setTasks] = useState<TaskSummary[]>([])
  const [loading, setLoading] = useState(true)
  const [fixTask, setFixTask] = useState<TaskSummary | null>(null)
  // Optimistic local removals so actions feel instant.
  const [ignored, setIgnored] = useState<Record<string, Set<string>>>({})
  const [dismissed, setDismissed] = useState<Set<string>>(new Set())

  const load = useCallback(() => {
    fetch('/api/tasks')
      .then(r => r.json())
      .then((res: TasksResponse) => { setTasks(res.tasks ?? []); setLoading(false) })
      .catch(() => setLoading(false))
  }, [])

  useEffect(() => { load() }, [load])

  // Apply optimistic ignores/dismissals on top of server data.
  const visibleIssues = useCallback((t: TaskSummary): HygieneIssue[] => {
    const ig = ignored[t.key]
    const issues = t.hygiene?.issues ?? []
    return ig ? issues.filter(i => !ig.has(i.code)) : issues
  }, [ignored])

  const unignore = useCallback((taskKey: string, code: string) => {
    setIgnored(prev => {
      const set = new Set(prev[taskKey] ?? [])
      set.delete(code)
      return { ...prev, [taskKey]: set }
    })
  }, [])

  const undismiss = useCallback((taskKey: string) => {
    setDismissed(prev => { const next = new Set(prev); next.delete(taskKey); return next })
  }, [])

  const ignore = useCallback((taskKey: string, code: string) => {
    setIgnored(prev => {
      const next = { ...prev }
      next[taskKey] = new Set(next[taskKey] ?? [])
      next[taskKey].add(code)
      return next
    })
    // Revert the optimistic hide if the server rejects it (e.g. must-fix code) or
    // the request fails — otherwise the issue silently reappears on next reload.
    fetch('/api/triage/ignore', {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ task_key: taskKey, code }),
    }).then(r => { if (!r.ok) unignore(taskKey, code) }).catch(() => unignore(taskKey, code))
  }, [unignore])

  const decide = useCallback((taskKey: string, body: Record<string, unknown>) => {
    setDismissed(prev => new Set(prev).add(taskKey))
    fetch('/api/triage/decision', {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ task_key: taskKey, ...body }),
    }).then(r => { if (!r.ok) undismiss(taskKey) }).catch(() => undismiss(taskKey))
  }, [undismiss])

  const later = useCallback((taskKey: string) => decide(taskKey, { decision: 'snoozed', snooze_days: 7 }), [decide])
  const keep = useCallback((taskKey: string) => decide(taskKey, { decision: 'keep' }), [decide])

  const groups = useMemo(() => {
    const live = tasks.filter(t => t.hygiene && !dismissed.has(t.key))
    const must: TaskSummary[] = []
    const nice: TaskSummary[] = []
    const review: TaskSummary[] = []
    for (const t of live) {
      const iss = visibleIssues(t)
      if (hasMustFix(iss)) must.push(t)
      else if (iss.length > 0) nice.push(t)
      else if (t.hygiene!.bucket === 'looks_stale' || t.hygiene!.bucket === 'not_sure') review.push(t)
    }
    return { must, nice, review }
  }, [tasks, dismissed, visibleIssues])

  if (loading) return <div className="p-10 text-sm" style={{ color: 'var(--ink-3)' }}>Reading your board…</div>

  const total = tasks.length
  const ready = total - (groups.must.length + groups.nice.length + groups.review.length)
  const attention = groups.must.length + groups.nice.length + groups.review.length
  const healthPct = total > 0 ? Math.round((ready / total) * 100) : 100
  const healthTone = healthPct >= 90 ? 'var(--success)' : healthPct >= 60 ? 'var(--accent)' : 'var(--warn)'

  return (
    <div className="space-y-10">
      {/* Header — same shape as Week / Tasks: kicker + title, big stat on the right */}
      <header className="rise flex items-end justify-between">
        <div>
          <p className="text-[11px] uppercase tracking-[0.2em]" style={{ color: 'var(--ink-3)' }}>Board health</p>
          <h1 className="type-title mt-1" style={{ color: 'var(--ink)' }}>Clean-up</h1>
        </div>
        <div className="text-right">
          <p className="font-mono tnum text-[32px] leading-none" style={{ color: healthTone }}>{healthPct}</p>
          <p className="text-[11px] mt-1.5" style={{ color: 'var(--ink-3)' }}>Board Score</p>
        </div>
      </header>

      {/* Summary tiles — the Week-view insight grid */}
      <div className="grid grid-cols-3 gap-6">
        <Insight kicker="Ready Work Items" value={ready} tone="var(--success)"
          body="Have everything Meridian needs to attribute your work." />
        <Insight kicker="Must fix" value={groups.must.length} tone="var(--warn)"
          body="Missing a due date, description, or clear title. Can't be ignored." />
        <Insight kicker="To tidy" value={groups.nice.length + groups.review.length} tone="var(--accent)"
          body="Optional hygiene, or stale tickets to review. Fix at leisure." />
      </div>

      {attention === 0 ? (
        <AllClear />
      ) : (
        <>
          <Section kicker="Can't track without these" title="Must fix" count={groups.must.length}
            blurb="Meridian can't track these accurately until they're cleared — these cannot be ignored.">
            {groups.must.map(t => <TicketCard key={t.key} task={t} issues={visibleIssues(t)} tone={TONE.must_fix}
              onFix={() => setFixTask(t)} />)}
          </Section>

          <Section kicker="Good hygiene" title="Nice to fix" count={groups.nice.length}
            blurb="An epic, labels, a priority or estimate. Fix, ignore, or come back later.">
            {groups.nice.map(t => <TicketCard key={t.key} task={t} issues={visibleIssues(t)} tone={TONE.optional}
              onFix={() => setFixTask(t)} onIgnore={code => ignore(t.key, code)} onLater={() => later(t.key)} />)}
          </Section>

          <Section kicker="Stale or unclear" title="Review" count={groups.review.length}
            blurb="Looks stale or unclear. Keep it, or open it to close in your tracker.">
            {groups.review.map(t => <TicketCard key={t.key} task={t} issues={[]} tone={TONE.review} review
              onFix={() => setFixTask(t)} onKeep={() => keep(t.key)} />)}
          </Section>
        </>
      )}

      {fixTask && <HygieneDialog task={fixTask} onClose={() => { setFixTask(null); load() }} onApplied={load} />}
    </div>
  )
}

// Insight tile — same Card vocabulary as WeekView's insight grid.
function Insight({ kicker, value, tone, body }: { kicker: string; value: number; tone: string; body: string }) {
  return (
    <Card className="p-5">
      <p className="text-[10px] uppercase tracking-[0.18em] mb-2" style={{ color: 'var(--ink-3)' }}>{kicker}</p>
      <p className="type-callout" style={{ color: value > 0 ? tone : 'var(--ink-4)' }}>{value}</p>
      <p className="text-[12px] mt-2 leading-relaxed" style={{ color: 'var(--ink-2)' }}>{body}</p>
    </Card>
  )
}

// A group of tickets under the shared SectionHead. Hidden when empty.
function Section({ kicker, title, count, blurb, children }: {
  kicker: string; title: string; count: number; blurb: string; children: React.ReactNode
}) {
  if (count === 0) return null
  return (
    <section>
      <SectionHead kicker={kicker} title={<>{title} <span style={{ color: 'var(--ink-3)' }}>· {count}</span></>} />
      <p className="text-[12px] mb-3 max-w-xl -mt-1.5" style={{ color: 'var(--ink-3)' }}>{blurb}</p>
      <div className="space-y-2.5">{children}</div>
    </section>
  )
}

function TicketCard({ task, issues, tone, review, onFix, onIgnore, onLater, onKeep }: {
  task: TaskSummary
  issues: HygieneIssue[]
  tone: string
  review?: boolean
  onFix: () => void
  onIgnore?: (code: string) => void
  onLater?: () => void
  onKeep?: () => void
}) {
  return (
    <div className="rounded-xl border overflow-hidden transition-colors"
      style={{ borderColor: 'var(--rule)', background: 'var(--surface)' }}>
      {/* Ticket header */}
      <div className="flex items-center gap-2.5 px-4 pt-3.5 pb-2.5">
        <ProviderGlyph provider={task.provider} size={18} />
        <TaskKey keyId={task.key} />
        <span className="text-[13px] font-medium truncate flex-1 min-w-0" style={{ color: 'var(--ink)' }}>{task.title}</span>
        {task.url && (
          <a href={task.url} target="_blank" rel="noopener noreferrer"
            className="text-[11px] shrink-0" style={{ color: 'var(--ink-4)' }}>Open ↗</a>
        )}
      </div>

      {/* Issue rows */}
      {issues.length > 0 && (
        <div className="px-4 pb-1">
          {issues.map(it => (
            <div key={it.code} className="flex items-center gap-2.5 py-1.5">
              <span className="w-1.5 h-1.5 rounded-full shrink-0"
                style={{ background: it.severity === 'must_fix' ? 'var(--warn)' : 'var(--accent)' }} />
              <span className="text-[12px] flex-1 min-w-0" style={{ color: 'var(--ink-2)' }}>{it.hint}</span>
              {it.severity === 'optional' && onIgnore && (
                <button onClick={() => onIgnore(it.code)}
                  className="text-[11px] px-1.5 py-0.5 rounded transition-colors shrink-0"
                  style={{ color: 'var(--ink-4)' }} title="Ignore this for this ticket">Ignore</button>
              )}
            </div>
          ))}
        </div>
      )}

      {review && (
        <p className="px-4 pb-1 text-[12px]" style={{ color: 'var(--ink-3)' }}>
          No recent activity, or no clear signal it&apos;s live.
        </p>
      )}

      {/* Card actions */}
      <div className="flex items-center gap-2 px-4 py-2.5 rule-t" style={{ borderColor: 'var(--rule)', background: 'var(--paper)' }}>
        <button onClick={onFix}
          className="text-[12px] px-3 py-1.5 rounded-md font-medium transition-colors"
          style={{ background: tone, color: '#fff' }}>
          {review ? 'Review' : 'Fix'} →
        </button>
        {onKeep && (
          <button onClick={onKeep} className="text-[12px] px-3 py-1.5 rounded-md border transition-colors"
            style={{ borderColor: 'var(--rule)', color: 'var(--ink-2)' }}>Keep</button>
        )}
        {onLater && (
          <button onClick={onLater} className="text-[12px] px-2.5 py-1.5 rounded-md transition-colors ml-auto"
            style={{ color: 'var(--ink-4)' }}>Later</button>
        )}
      </div>
    </div>
  )
}

function AllClear() {
  return (
    <div className="py-16 text-center rounded-xl border" style={{ borderColor: 'var(--rule)', background: 'var(--surface)' }}>
      <p className="type-empty" style={{ color: 'var(--ink-2)' }}>Your board is healthy.</p>
      <p className="text-[12px] mt-2" style={{ color: 'var(--ink-3)' }}>Every ticket has what Meridian needs. Nothing to clean up.</p>
    </div>
  )
}
