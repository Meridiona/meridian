// meridian — normalises screenpipe activity into structured app sessions
'use client'

import { useEffect, useState } from 'react'
import { fmtDur, fmtClock, CATS, AppGlyph, CatDot, TaskKey } from '@/components/atoms'
import type { QueueItem } from '@/app/api/queue-review/route'

export default function QueueView() {
  const [items, setItems] = useState<QueueItem[]>([])
  const [loading, setLoading] = useState(true)
  const [expanded, setExpanded] = useState<number | null>(null)

  useEffect(() => {
    fetch('/api/queue-review')
      .then(r => r.json())
      .then(d => { setItems(d.items ?? []); setLoading(false) })
  }, [])

  function dismiss(id: number) {
    setItems(xs => xs.filter(s => s.id !== id))
  }

  if (loading) {
    return (
      <div className="space-y-8">
        <header className="rise">
          <p className="text-[11px] uppercase tracking-[0.2em]" style={{ color: 'var(--ink-3)' }}>Review queue</p>
          <h1 className="font-serif text-[56px] leading-[1] tracking-tight mt-1" style={{ color: 'var(--ink)' }}>Help me get this right</h1>
        </header>
        <p className="text-[13px]" style={{ color: 'var(--ink-3)' }}>Loading…</p>
      </div>
    )
  }

  return (
    <div className="space-y-8">
      <header className="rise flex items-end justify-between">
        <div>
          <p className="text-[11px] uppercase tracking-[0.2em]" style={{ color: 'var(--ink-3)' }}>Review queue</p>
          <h1 className="font-serif text-[56px] leading-[1] tracking-tight mt-1" style={{ color: 'var(--ink)' }}>
            Help me get this right
          </h1>
          <p className="mt-3 text-[14px] max-w-prose" style={{ color: 'var(--ink-2)' }}>
            Sessions where the classifier wasn&apos;t confident. One click per session — your judgements train the model.
          </p>
        </div>
        <div className="text-right">
          <p className="font-mono tnum text-[40px] leading-none" style={{ color: 'var(--ink)' }}>{items.length}</p>
          <p className="text-[11px] mt-1.5" style={{ color: 'var(--ink-3)' }}>waiting</p>
        </div>
      </header>

      {items.length === 0 ? (
        <div className="py-16 text-center rule rounded-xl border" style={{ borderColor: 'var(--rule)', background: 'var(--surface)' }}>
          <p className="font-serif italic text-[24px]" style={{ color: 'var(--ink-2)' }}>Inbox zero.</p>
          <p className="text-[12px] mt-2" style={{ color: 'var(--ink-3)' }}>
            Nothing waiting. We&apos;ll surface new items here as confidence drops.
          </p>
        </div>
      ) : (
        <div className="rule rounded-xl border overflow-hidden" style={{ borderColor: 'var(--rule)' }}>
          {items.map((s, i) => (
            <QueueRow
              key={s.id}
              session={s}
              first={i === 0}
              expanded={expanded === s.id}
              onToggle={() => setExpanded(x => x === s.id ? null : s.id)}
              onDismiss={() => dismiss(s.id)}
            />
          ))}
        </div>
      )}
    </div>
  )
}

function QueueRow({
  session, first, expanded, onToggle, onDismiss,
}: {
  session: QueueItem
  first: boolean
  expanded: boolean
  onToggle: () => void
  onDismiss: () => void
}) {
  return (
    <div className={first ? '' : 'rule-t'} style={{ borderTopColor: 'var(--rule)', background: 'var(--surface)' }}>
      <div className="px-5 py-4 transition-colors" style={{ background: expanded ? 'var(--surface-2)' : 'var(--surface)' }}>
        <button onClick={onToggle}
          className="w-full text-left grid grid-cols-[auto_1fr_auto] gap-5 items-center">
          <AppGlyph app={session.app} size={28} />
          <div className="min-w-0">
            <p className="text-[14px] truncate" style={{ color: 'var(--ink)' }}>{session.titles[0] || session.app}</p>
            <div className="flex items-center gap-2 mt-1">
              <span className="font-mono tnum text-[11px]" style={{ color: 'var(--ink-3)' }}>{fmtClock(session.started_at)}</span>
              <CatDot cat={session.cat} />
              <span className="text-[11px]" style={{ color: 'var(--ink-3)' }}>{CATS[session.cat]?.label ?? session.cat}</span>
              <span className="text-[11px]" style={{ color: 'var(--ink-4)' }}>·</span>
              <span className="text-[11px]" style={{ color: 'var(--ink-3)' }}>{fmtDur(session.dur)}</span>
            </div>
          </div>
          <span className="text-[11px]" style={{ color: 'var(--ink-4)' }}>{expanded ? '−' : '+'}</span>
        </button>

        <div className="flex flex-wrap items-center gap-2 mt-3 pl-[44px]">
          <span className="text-[10px] uppercase tracking-[0.16em] mr-1" style={{ color: 'var(--ink-3)' }}>Candidates →</span>
          {session.candidates.map(c => (
            <button key={c}
              onClick={(e) => { e.stopPropagation(); onDismiss() }}
              className="px-2.5 py-1.5 rounded-md text-[11px] transition-colors hover:opacity-80"
              style={{ background: 'var(--tint)', border: '1px solid var(--rule-2)' }}>
              <TaskKey keyId={c} />
            </button>
          ))}
          <button onClick={(e) => { e.stopPropagation(); onDismiss() }}
            className="text-[11px] px-2 py-1.5 rounded-md" style={{ color: 'var(--ink-3)', border: '1px solid var(--rule-2)' }}>
            Overhead
          </button>
        </div>
      </div>

      {expanded && (
        <div className="px-5 pb-5 rule-t" style={{ borderTopColor: 'var(--rule)' }}>
          <div className="pt-4">
            {session.explain && <>
              <p className="text-[10px] uppercase tracking-[0.16em] mb-2" style={{ color: 'var(--ink-3)' }}>Why ambiguous</p>
              <p className="text-[13px] mb-4" style={{ color: 'var(--ink-2)' }}>{session.explain}</p>
            </>}
            <p className="text-[10px] uppercase tracking-[0.16em] mb-2" style={{ color: 'var(--ink-3)' }}>Windows seen</p>
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
      )}
    </div>
  )
}
