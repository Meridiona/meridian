//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

import { useCallback, useEffect, useMemo, useState } from 'react'
import { TaskKey, ProviderGlyph } from '@/components/atoms'
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

  const ignore = useCallback((taskKey: string, code: string) => {
    setIgnored(prev => {
      const next = { ...prev }
      next[taskKey] = new Set(next[taskKey] ?? [])
      next[taskKey].add(code)
      return next
    })
    fetch('/api/triage/ignore', {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ task_key: taskKey, code }),
    }).catch(() => load())
  }, [load])

  const later = useCallback((taskKey: string) => {
    setDismissed(prev => new Set(prev).add(taskKey))
    fetch('/api/triage/decision', {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ task_key: taskKey, decision: 'snoozed', snooze_days: 7 }),
    }).catch(() => load())
  }, [load])

  const keep = useCallback((taskKey: string) => {
    setDismissed(prev => new Set(prev).add(taskKey))
    fetch('/api/triage/decision', {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ task_key: taskKey, decision: 'keep' }),
    }).catch(() => load())
  }, [load])

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

  return (
    <div className="max-w-3xl mx-auto px-6 md:px-8 py-8 rise">
      {/* Header */}
      <header className="mb-8">
        <p className="text-[11px] uppercase tracking-[0.2em] mb-1" style={{ color: 'var(--ink-3)' }}>Board health</p>
        <h1 className="type-title" style={{ color: 'var(--ink)' }}>Clean-up</h1>
        <p className="text-[13px] mt-2 max-w-prose" style={{ color: 'var(--ink-3)' }}>
          A healthy board means Meridian attributes your work accurately. Must-fix items come first;
          the rest you can tidy, ignore, or leave for later.
        </p>
      </header>

      {/* Health summary */}
      <div className="flex items-stretch gap-4 mb-9">
        <HealthRing pct={healthPct} />
        <div className="grid grid-cols-3 flex-1 rounded-xl border overflow-hidden" style={{ borderColor: 'var(--rule)' }}>
          <Stat label="Ready" value={ready} tone="var(--success)" />
          <Stat label="Must fix" value={groups.must.length} tone="var(--warn)" border />
          <Stat label="To tidy" value={groups.nice.length + groups.review.length} tone="var(--accent)" border />
        </div>
      </div>

      {attention === 0 ? (
        <AllClear />
      ) : (
        <div className="space-y-9">
          <Section title="Must fix" tone={TONE.must_fix}
            blurb="Meridian can't track these accurately until they're cleared — these cannot be ignored."
            tasks={groups.must}>
            {t => <TicketCard key={t.key} task={t} issues={visibleIssues(t)} tone={TONE.must_fix}
              onFix={() => setFixTask(t)} />}
          </Section>

          <Section title="Nice to fix" tone={TONE.optional}
            blurb="Good hygiene — an epic, labels, a priority or estimate. Fix, ignore, or come back later."
            tasks={groups.nice}>
            {t => <TicketCard key={t.key} task={t} issues={visibleIssues(t)} tone={TONE.optional}
              onFix={() => setFixTask(t)} onIgnore={code => ignore(t.key, code)} onLater={() => later(t.key)} />}
          </Section>

          <Section title="Review" tone={TONE.review}
            blurb="Looks stale or unclear. Keep it, or open it to close in your tracker."
            tasks={groups.review}>
            {t => <TicketCard key={t.key} task={t} issues={[]} tone={TONE.review} review
              onFix={() => setFixTask(t)} onKeep={() => keep(t.key)} />}
          </Section>
        </div>
      )}

      {fixTask && <HygieneDialog task={fixTask} onClose={() => { setFixTask(null); load() }} />}
    </div>
  )
}

function HealthRing({ pct }: { pct: number }) {
  const size = 84, r = size / 2 - 7, c = 2 * Math.PI * r
  const tone = pct >= 90 ? 'var(--success)' : pct >= 60 ? 'var(--accent)' : 'var(--warn)'
  return (
    <div className="relative shrink-0" style={{ width: size, height: size }}>
      <svg width={size} height={size} viewBox={`0 0 ${size} ${size}`}>
        <circle cx={size / 2} cy={size / 2} r={r} fill="none" stroke="var(--rule-2)" strokeWidth="6" />
        <circle cx={size / 2} cy={size / 2} r={r} fill="none" stroke={tone} strokeWidth="6" strokeLinecap="round"
          strokeDasharray={`${(c * pct) / 100} ${c}`} transform={`rotate(-90 ${size / 2} ${size / 2})`} />
      </svg>
      <div className="absolute inset-0 flex flex-col items-center justify-center">
        <span className="type-stat tnum" style={{ color: 'var(--ink)', fontSize: 20 }}>{pct}</span>
        <span className="text-[9px] uppercase tracking-[0.14em]" style={{ color: 'var(--ink-3)' }}>healthy</span>
      </div>
    </div>
  )
}

function Stat({ label, value, tone, border }: { label: string; value: number; tone: string; border?: boolean }) {
  return (
    <div className={`px-4 py-4 flex flex-col justify-center ${border ? 'rule-l' : ''}`} style={{ borderLeftColor: 'var(--rule)' }}>
      <span className="type-stat tnum" style={{ color: value > 0 ? tone : 'var(--ink-4)', fontSize: 24 }}>{value}</span>
      <span className="text-[10px] uppercase tracking-[0.14em] mt-1" style={{ color: 'var(--ink-3)' }}>{label}</span>
    </div>
  )
}

function Section({ title, tone, blurb, tasks, children }: {
  title: string; tone: string; blurb: string; tasks: TaskSummary[]
  children: (t: TaskSummary) => React.ReactNode
}) {
  if (tasks.length === 0) return null
  return (
    <section>
      <div className="flex items-center gap-2 mb-1">
        <span className="w-2 h-2 rounded-full" style={{ background: tone }} />
        <h2 className="text-[15px] font-medium" style={{ color: 'var(--ink)' }}>
          {title} <span style={{ color: 'var(--ink-3)' }}>· {tasks.length}</span>
        </h2>
      </div>
      <p className="text-[12px] mb-3 max-w-xl" style={{ color: 'var(--ink-3)' }}>{blurb}</p>
      <div className="space-y-2.5">{tasks.map(children)}</div>
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
    <div className="rounded-2xl border p-10 text-center" style={{ background: 'var(--surface)', borderColor: 'var(--rule)' }}>
      <div aria-hidden style={{ fontSize: 34 }} className="mb-2">✨</div>
      <h2 className="text-[18px] font-medium mb-1" style={{ color: 'var(--ink)' }}>Your board is healthy</h2>
      <p className="text-sm" style={{ color: 'var(--ink-3)' }}>Every ticket has what Meridian needs. Nothing to clean up.</p>
    </div>
  )
}
