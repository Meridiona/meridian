//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

import { useState, useEffect } from 'react'
import { fmtDur, fmtClock, CATS, AppGlyph, CatDot } from '@/components/atoms'
import TaskBadge from '@/components/TaskBadge'
import { load } from '@/lib/bridge'
import type { TodayResponse } from '@/lib/api-types'

type Session = TodayResponse['sessions'][number]

export default function SessionsView() {
  const [data, setData] = useState<TodayResponse | null>(null)
  const [filter, setFilter] = useState<string>('all')

  useEffect(() => {
    // get_today (Rust) — the sessions list rides the Today payload.
    load<TodayResponse>('/api/today', 'get_today').then(setData).catch(() => {})
  }, [])

  if (!data) {
    return <div className="space-y-8">
      <header className="rise">
        <p className="text-[11px] uppercase tracking-[0.2em]" style={{ color: 'var(--ink-3)' }}>Sessions</p>
        <h1 className="type-title mt-1" style={{ color: 'var(--ink)' }}>Every moment, captured</h1>
      </header>
      <p className="text-[13px]" style={{ color: 'var(--ink-3)' }}>Loading…</p>
    </div>
  }

  const sorted = [...data.sessions].sort((a, b) =>
    new Date(b.started_at).getTime() - new Date(a.started_at).getTime()
  )
  const filtered = filter === 'all' ? sorted : sorted.filter(s => s.cat === filter)
  const cats = Array.from(new Set(sorted.map(s => s.cat)))

  return (
    <div className="space-y-8">
      <header className="rise flex items-end justify-between">
        <div>
          <p className="text-[11px] uppercase tracking-[0.2em]" style={{ color: 'var(--ink-3)' }}>Sessions</p>
          <h1 className="type-title mt-1" style={{ color: 'var(--ink)' }}>
            Every moment, captured
          </h1>
        </div>
        <p className="text-[12px]" style={{ color: 'var(--ink-3)' }}>
          <span className="font-mono tnum">{sorted.length}</span> sessions today
        </p>
      </header>

      {/* Category filter chips */}
      <div className="flex flex-wrap gap-2">
        <button onClick={() => setFilter('all')}
          className="px-3 py-1.5 rounded-full text-[12px] transition-colors"
          style={{
            background: filter === 'all' ? 'var(--ink)' : 'transparent',
            color: filter === 'all' ? 'var(--paper)' : 'var(--ink-2)',
            border: '1px solid ' + (filter === 'all' ? 'var(--ink)' : 'var(--rule-2)'),
          }}>
          All
        </button>
        {cats.map(c => (
          <button key={c} onClick={() => setFilter(c)}
            className="px-3 py-1.5 rounded-full text-[12px] inline-flex items-center gap-1.5 transition-colors"
            style={{
              background: filter === c ? 'var(--surface-2)' : 'transparent',
              color: 'var(--ink-2)',
              border: '1px solid ' + (filter === c ? 'var(--ink-3)' : 'var(--rule-2)'),
            }}>
            <CatDot cat={c} />
            {CATS[c]?.label ?? c}
          </button>
        ))}
      </div>

      {filtered.length === 0 ? (
        <div className="py-12 text-center rule rounded-xl border" style={{ borderColor: 'var(--rule)', background: 'var(--surface)' }}>
          <p className="text-[13px]" style={{ color: 'var(--ink-3)' }}>No sessions match this filter.</p>
        </div>
      ) : (
        <div className="rule rounded-xl border overflow-hidden" style={{ borderColor: 'var(--rule)' }}>
          {filtered.map((s, i) => <SessionRow key={s.id} session={s} first={i === 0} />)}
        </div>
      )}
    </div>
  )
}

function SessionRow({ session, first }: { session: Session; first: boolean }) {
  const [open, setOpen] = useState(false)
  return (
    <div className={first ? '' : 'rule-t'} style={{ borderTopColor: 'var(--rule)', background: 'var(--surface)' }}>
      <button onClick={() => setOpen(o => !o)}
        className="w-full text-left grid grid-cols-[80px_auto_1fr_auto_auto] gap-5 items-center px-5 py-3.5">
        <span className="font-mono tnum text-[12px]" style={{ color: 'var(--ink-3)' }}>
          {fmtClock(session.started_at)}
        </span>
        <AppGlyph app={session.app} size={22} />
        <div className="min-w-0">
          <p className="text-[13px] truncate" style={{ color: 'var(--ink)' }}>{session.titles[0] || session.app}</p>
          <div className="flex items-center gap-2 mt-0.5">
            <CatDot cat={session.cat} />
            <span className="text-[11px]" style={{ color: 'var(--ink-3)' }}>{CATS[session.cat]?.label ?? session.cat}</span>
            {session.routing === 'queue' && (
              <span className="text-[11px]" style={{ color: 'var(--accent)' }}>· needs review</span>
            )}
          </div>
        </div>
        <span className="font-mono tnum text-[12px]" style={{ color: 'var(--ink-2)' }}>{fmtDur(session.dur)}</span>
        <div className="min-w-[60px] flex justify-end">
          <TaskBadge
            taskKey={session.task_key}
            sessionType={session.session_type}
            routing={session.routing}
            confidence={session.link_confidence}
            method={session.link_method}
            size="xs"
          />
        </div>
      </button>

      {open && (
        <div className="px-5 pb-4 rule-t" style={{ borderTopColor: 'var(--rule)' }}>
          <div className="grid grid-cols-2 gap-6 pt-4">
            <div>
              {session.explain && <>
                <p className="text-[10px] uppercase tracking-[0.16em] mb-2" style={{ color: 'var(--ink-3)' }}>Why this category</p>
                <p className="text-[12px]" style={{ color: 'var(--ink-2)' }}>{session.explain}</p>
              </>}
            </div>
            <div>
              <p className="text-[10px] uppercase tracking-[0.16em] mb-2" style={{ color: 'var(--ink-3)' }}>Windows</p>
              <div className="flex flex-wrap gap-1.5">
                {session.titles.map(t => (
                  <span key={t} className="font-mono text-[11px] px-2 py-1 rounded-md tnum"
                    style={{ background: 'var(--surface-2)', border: '1px solid var(--rule)', color: 'var(--ink-2)' }}>
                    {t}
                  </span>
                ))}
              </div>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}
