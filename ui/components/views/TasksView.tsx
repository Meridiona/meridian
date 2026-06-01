// meridian — normalises screenpipe activity into structured app sessions
'use client'

import { useEffect, useState } from 'react'
import { fmtDur, fmtClock, AppGlyph, CatDot, TaskKey, StatusPill, SegBar, SectionHead, Card, CATS } from '@/components/atoms'
import type { TaskSummary, TasksResponse } from '@/app/api/tasks/route'
import type { TodayResponse } from '@/app/api/today/route'

export default function TasksView({ focusKey }: { focusKey?: string | null }) {
  const [data, setData] = useState<TasksResponse | null>(null)
  const [todaySessions, setTodaySessions] = useState<TodayResponse['sessions']>([])
  const [selected, setSelected] = useState<string | null>(focusKey ?? null)

  useEffect(() => {
    fetch('/api/tasks').then(r => r.json()).then((d: TasksResponse) => {
      setData(d)
      if (!selected && d.tasks.length > 0) {
        const first = d.tasks.find(t => t.today_s > 0) ?? d.tasks[0]
        setSelected(first.key)
      }
    }).catch(() => {})
    fetch('/api/today').then(r => r.json()).then((d: TodayResponse) => {
      setTodaySessions(d.sessions ?? [])
    }).catch(() => {})
  }, []) // eslint-disable-line react-hooks/exhaustive-deps

  if (!data) {
    return (
      <div className="space-y-8">
        <header className="rise">
          <p className="text-[11px] uppercase tracking-[0.2em]" style={{ color: 'var(--ink-3)' }}>Tasks</p>
          <h1 className="font-serif text-[56px] leading-[1] tracking-tight mt-1" style={{ color: 'var(--ink)' }}>What you&apos;re working on</h1>
        </header>
        <p className="text-[13px]" style={{ color: 'var(--ink-3)' }}>Loading…</p>
      </div>
    )
  }

  if (data.tasks.length === 0) {
    return (
      <div className="space-y-8">
        <header className="rise">
          <p className="text-[11px] uppercase tracking-[0.2em]" style={{ color: 'var(--ink-3)' }}>Tasks</p>
          <h1 className="font-serif text-[56px] leading-[1] tracking-tight mt-1" style={{ color: 'var(--ink)' }}>What you&apos;re working on</h1>
        </header>
        <div className="py-16 text-center rounded-xl border" style={{ borderColor: 'var(--rule)', background: 'var(--surface)' }}>
          <p className="text-[13px]" style={{ color: 'var(--ink-3)' }}>No tasks synced yet.</p>
          <p className="text-[11px] mt-1" style={{ color: 'var(--ink-4)' }}>Connect Jira, Linear, or GitHub to see your tasks here.</p>
        </div>
      </div>
    )
  }

  const sel = data.tasks.find(t => t.key === selected) ?? data.tasks[0]
  const touched = data.tasks.filter(t => t.today_s > 0).length

  return (
    <div className="space-y-8">
      <header className="rise flex items-end justify-between">
        <div>
          <p className="text-[11px] uppercase tracking-[0.2em]" style={{ color: 'var(--ink-3)' }}>Tasks</p>
          <h1 className="font-serif text-[56px] leading-[1] tracking-tight mt-1" style={{ color: 'var(--ink)' }}>
            What you&apos;re working on
          </h1>
        </div>
        <p className="text-[12px]" style={{ color: 'var(--ink-3)' }}>
          <span className="font-mono tnum">{touched}</span> touched today
          <span className="mx-2">·</span>
          <span className="font-mono tnum">{data.tasks.length}</span> on board
        </p>
      </header>

      <div className="grid grid-cols-1 lg:grid-cols-[minmax(0,300px)_minmax(0,1fr)] gap-8">
        {/* Task list */}
        <div className="space-y-px rule rounded-xl overflow-hidden border" style={{ borderColor: 'var(--rule)' }}>
          {data.tasks.map(t => (
            <TaskRow key={t.key} task={t} selected={t.key === selected} onSelect={() => setSelected(t.key)} />
          ))}
        </div>

        {/* Task detail */}
        {sel && <TaskDetail task={sel} sessions={todaySessions.filter(s => s.task_key === sel.key)} />}
      </div>
    </div>
  )
}

function TaskRow({ task, selected, onSelect }: { task: TaskSummary; selected: boolean; onSelect: () => void }) {
  const segs = Object.entries(task.cats).map(([cat, value]) => ({ cat, value }))
  return (
    <button onClick={onSelect}
      className="w-full text-left px-4 py-3 transition-colors"
      style={{
        background: selected ? 'var(--surface-2)' : 'var(--surface)',
        borderLeft: selected ? '2px solid var(--accent)' : '2px solid transparent',
      }}>
      <div className="flex items-center gap-3">
        <TaskKey keyId={task.key} />
        <StatusPill status={task.status} />
        <span className="ml-auto font-mono tnum text-[12px]" style={{ color: task.today_s > 0 ? 'var(--ink)' : 'var(--ink-4)' }}>
          {task.today_s > 0 ? fmtDur(task.today_s) : '—'}
        </span>
      </div>
      <p className="text-[13px] mt-1.5 truncate" style={{ color: 'var(--ink)' }}>{task.title}</p>
      <div className="mt-1.5">
        <SegBar
          segments={segs.length ? segs : [{ value: 1, color: 'var(--rule-2)' }]}
          height={2}
        />
      </div>
    </button>
  )
}

function TaskDetail({ task, sessions }: { task: TaskSummary; sessions: TodayResponse['sessions'] }) {
  const sortedSessions = [...sessions].sort((a, b) => a.started_at.localeCompare(b.started_at))

  return (
    <div className="space-y-7 min-w-0">
      <div>
        <div className="flex items-center gap-3 mb-3">
          <TaskKey keyId={task.key} big />
          <StatusPill status={task.status} />
          <span className="text-[11px]" style={{ color: 'var(--ink-3)' }}>{task.provider}</span>
          {task.url && (
            <a href={task.url} target="_blank" rel="noopener noreferrer"
              className="ml-auto text-[12px]" style={{ color: 'var(--ink-3)' }}>
              Open ↗
            </a>
          )}
        </div>
        <h2 className="font-serif text-[36px] leading-[1.1] tracking-tight" style={{ color: 'var(--ink)' }}>
          {task.title}
        </h2>
        {task.description && (
          <p className="text-[14px] mt-3 max-w-prose" style={{ color: 'var(--ink-2)' }}>{task.description}</p>
        )}
      </div>

      <div className="grid grid-cols-3 rule-t rule-b" style={{ borderColor: 'var(--rule)' }}>
        <div className="px-5 py-4">
          <p className="text-[10px] uppercase tracking-[0.16em] mb-2" style={{ color: 'var(--ink-3)' }}>Today</p>
          <p className="font-mono tnum text-[22px] leading-none" style={{ color: 'var(--ink)' }}>{fmtDur(task.today_s)}</p>
          {task.today_autonomous_s >= 60 && (
            <p className="text-[10px] mt-1.5" style={{ color: 'var(--live)' }}
              title="Of today's total, the agent ran on its own while you were away — the part that adds time beyond your own.">
              +{fmtDur(task.today_autonomous_s)} agent while away
            </p>
          )}
        </div>
        <div className="px-5 py-4 rule-l" style={{ borderLeftColor: 'var(--rule)' }}>
          <p className="text-[10px] uppercase tracking-[0.16em] mb-2" style={{ color: 'var(--ink-3)' }}>This week</p>
          <p className="font-mono tnum text-[22px] leading-none" style={{ color: 'var(--ink)' }}>{fmtDur(task.week_s)}</p>
        </div>
        <div className="px-5 py-4 rule-l" style={{ borderLeftColor: 'var(--rule)' }}>
          <p className="text-[10px] uppercase tracking-[0.16em] mb-2" style={{ color: 'var(--ink-3)' }}>Sessions</p>
          <p className="font-mono tnum text-[22px] leading-none" style={{ color: 'var(--ink)' }}>{task.session_count}</p>
        </div>
      </div>

      {sortedSessions.length > 0 ? (
        <div>
          <p className="text-[10px] uppercase tracking-[0.16em] mb-3" style={{ color: 'var(--ink-3)' }}>Sessions today</p>
          <div className="rule rounded-xl border overflow-hidden" style={{ borderColor: 'var(--rule)' }}>
            {sortedSessions.map((s, i) => (
              <div key={s.id}
                className={`grid grid-cols-[auto_1fr_auto] items-center gap-4 px-4 py-3 ${i > 0 ? 'rule-t' : ''}`}
                style={{ borderTopColor: 'var(--rule)', background: 'var(--surface)' }}>
                <AppGlyph app={s.app} size={22} />
                <div className="min-w-0">
                  <p className="text-[13px] truncate" style={{ color: 'var(--ink)' }}>{s.titles[0] || s.app}</p>
                  <div className="flex items-center gap-2 mt-0.5">
                    <span className="font-mono tnum text-[11px]" style={{ color: 'var(--ink-3)' }}>{fmtClock(s.started_at)}</span>
                    <CatDot cat={s.cat} />
                    <span className="text-[11px]" style={{ color: 'var(--ink-3)' }}>{CATS[s.cat]?.label ?? s.cat}</span>
                  </div>
                </div>
                <span className="font-mono tnum text-[12px]" style={{ color: 'var(--ink-2)' }}>{fmtDur(s.dur)}</span>
              </div>
            ))}
          </div>
        </div>
      ) : task.today_s === 0 ? (
        <div className="py-12 text-center rule rounded-xl border" style={{ borderColor: 'var(--rule)', background: 'var(--surface)' }}>
          <p className="text-[13px]" style={{ color: 'var(--ink-3)' }}>No activity captured for this task today.</p>
        </div>
      ) : null}

      {task.today_s > 0 && (
        <Card className="p-5">
          <SectionHead kicker="Suggested worklog" title={`Log ${fmtDur(task.today_s)} to ${task.key}`} />
          <div className="flex items-center gap-3 mt-3">
            <button className="text-[12px] px-3 py-1.5 rounded-md font-medium"
              style={{ color: 'var(--paper)', background: 'var(--ink)' }}>
              Log to {task.provider === 'jira' ? 'Jira' : task.provider}
            </button>
            <button className="text-[12px] px-3 py-1.5 rounded-md" style={{ color: 'var(--ink-3)' }}>
              Edit draft
            </button>
          </div>
        </Card>
      )}
    </div>
  )
}

// keep this export for compat
export { AppGlyph }
