//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { TaskKey, ProviderGlyph } from '@/components/atoms'
import type { TriageTicket, TriageResponse, TriageBucket } from '@/app/api/triage/route'

// Why a clean board matters — rotated through while the user works so the 10
// minutes feels like learning, not chores. Each ties back to attribution accuracy.
const FACTS: string[] = [
  'Meridian matches your real work to these tickets. The cleaner they are, the more accurate your auto-worklog.',
  'A ticket with no description is one your future self — and your AI — can never match work to.',
  "Most boards carry a long tail of tickets nobody will touch again. Clearing them sharpens every suggestion.",
  'Specific titles beat clever ones: “Integrate Stripe Checkout” attributes; “Fix bug” doesn’t.',
  'Fewer, cleaner candidates make a local model far more accurate than a giant noisy backlog.',
  'Ten minutes here saves a wrong worklog at 6pm — when you’d rather be done for the day.',
  'Closing a dead ticket is a feature, not a failure. It’s one less thing pretending to be work.',
  'Due dates and sprints tell Meridian what’s live now — so it focuses on what you’re actually doing.',
]

const BUCKET_META: Record<Exclude<TriageBucket, 'ready'>, { label: string; blurb: string; tone: string }> = {
  needs_detail: {
    label: 'Could use more detail',
    blurb: "Likely active, but too thin for Meridian to attribute work to. Add a line in your tracker, or keep it.",
    tone: 'var(--accent)',
  },
  looks_stale: {
    label: 'Looks stale',
    blurb: 'No recent movement and nothing says it’s live. Exclude it from matching, or keep it if it’s still real.',
    tone: 'var(--warn)',
  },
  not_sure: {
    label: 'Worth a glance',
    blurb: "Open and reasonable, but no clear signal either way. A quick keep or exclude is all it needs.",
    tone: 'var(--ink-3)',
  },
}

type Decision = 'keep' | 'excluded' | 'snoozed'

export default function BoardCleanupView() {
  const [items, setItems] = useState<TriageTicket[]>([])
  const [counts, setCounts] = useState<TriageResponse['counts'] | null>(null)
  const [hasRun, setHasRun] = useState(true)
  const [loading, setLoading] = useState(true)
  const [decided, setDecided] = useState(0)
  const [factIdx, setFactIdx] = useState(0)
  const startTotal = useRef<number | null>(null)

  const load = useCallback(() => {
    fetch('/api/triage')
      .then(r => r.json())
      .then((res: TriageResponse) => {
        const attention = (res.items ?? []).filter(i => i.bucket !== 'ready' && !i.decision)
        setItems(attention)
        setCounts(res.counts)
        setHasRun(res.has_run)
        if (startTotal.current === null) startTotal.current = attention.length
        setLoading(false)
      })
      .catch(() => setLoading(false))
  }, [])

  useEffect(() => { load() }, [load])

  // Rotate the facts gently.
  useEffect(() => {
    const id = setInterval(() => setFactIdx(i => (i + 1) % FACTS.length), 7000)
    return () => clearInterval(id)
  }, [])

  const decide = useCallback((taskKey: string, decision: Decision, openUrl?: string) => {
    if (openUrl) window.open(openUrl, '_blank', 'noopener')
    // Optimistic: clear the card immediately so the flow feels instant.
    setItems(prev => prev.filter(i => i.task_key !== taskKey))
    setDecided(d => d + 1)
    fetch('/api/triage/decision', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ task_key: taskKey, decision }),
    }).then(r => {
      if (!r.ok) { load() } // resync on failure — never silently lose a decision
    }).catch(() => load())
  }, [load])

  const keepAll = useCallback((bucket?: TriageBucket) => {
    const targets = items.filter(i => !bucket || i.bucket === bucket)
    setItems(prev => prev.filter(i => bucket ? i.bucket !== bucket : false))
    setDecided(d => d + targets.length)
    Promise.all(targets.map(t =>
      fetch('/api/triage/decision', {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ task_key: t.task_key, decision: 'keep' }),
      }),
    )).catch(() => load())
  }, [items, load])

  const grouped = useMemo(() => {
    const g: Record<string, TriageTicket[]> = {}
    for (const it of items) (g[it.bucket] ??= []).push(it)
    return g
  }, [items])

  if (loading) {
    return <div className="p-8 text-sm" style={{ color: 'var(--ink-3)' }}>Reading your board…</div>
  }

  const total = counts?.total ?? 0
  const readyCount = counts?.ready ?? 0
  const remaining = items.length
  const initial = startTotal.current ?? 0
  const progressPct = initial > 0 ? Math.round(((initial - remaining) / initial) * 100) : 100

  // No tracker synced yet.
  if (!hasRun || total === 0) {
    return (
      <div className="max-w-2xl mx-auto p-8">
        <h1 className="text-[22px] font-medium mb-2" style={{ color: 'var(--ink)' }}>Tidy your board</h1>
        <p className="text-sm" style={{ color: 'var(--ink-3)' }}>
          Connect a tracker and sync your tickets — then come back here to clean them up in a couple of minutes.
        </p>
      </div>
    )
  }

  return (
    <div className="max-w-3xl mx-auto p-6 md:p-8">
      {/* Header + progress */}
      <header className="mb-6">
        <h1 className="text-[22px] font-medium mb-1" style={{ color: 'var(--ink)' }}>Tidy your board</h1>
        <p className="text-sm mb-4" style={{ color: 'var(--ink-3)' }}>
          A quick pass so Meridian attributes your work accurately. {readyCount} of {total} already look great.
        </p>
        <div className="flex items-center gap-3">
          <div className="flex-1 h-1.5 rounded-full overflow-hidden" style={{ background: 'var(--rule)' }}>
            <div className="h-full rounded-full transition-all duration-500"
              style={{ width: `${progressPct}%`, background: 'var(--success)' }} />
          </div>
          <span className="text-xs tnum" style={{ color: 'var(--ink-3)' }}>
            {initial - remaining}/{initial}
          </span>
        </div>
      </header>

      {/* Fun fact strip */}
      <div className="rounded-xl border px-4 py-3 mb-6 flex items-start gap-3"
        style={{ background: 'var(--tint)', borderColor: 'var(--rule)' }}>
        <span aria-hidden style={{ fontSize: 15 }}>💡</span>
        <p className="text-[13px] leading-relaxed transition-opacity duration-300" style={{ color: 'var(--ink-2)' }}>
          {FACTS[factIdx]}
        </p>
      </div>

      {remaining === 0 ? (
        <AllClear total={total} readyCount={readyCount} touched={decided} />
      ) : (
        (['needs_detail', 'looks_stale', 'not_sure'] as const).map(bucket => {
          const list = grouped[bucket]
          if (!list || list.length === 0) return null
          const meta = BUCKET_META[bucket]
          return (
            <section key={bucket} className="mb-8">
              <div className="flex items-end justify-between mb-2">
                <div>
                  <div className="flex items-center gap-2 mb-1">
                    <span className="w-2 h-2 rounded-full" style={{ background: meta.tone }} />
                    <h2 className="text-[15px] font-medium" style={{ color: 'var(--ink)' }}>
                      {meta.label} <span style={{ color: 'var(--ink-3)' }}>· {list.length}</span>
                    </h2>
                  </div>
                  <p className="text-[12px] max-w-xl" style={{ color: 'var(--ink-3)' }}>{meta.blurb}</p>
                </div>
                <button onClick={() => keepAll(bucket)}
                  className="text-[12px] px-2.5 py-1 rounded-md border whitespace-nowrap shrink-0 transition-colors"
                  style={{ borderColor: 'var(--rule)', color: 'var(--ink-2)' }}>
                  Keep all
                </button>
              </div>
              <div className="space-y-2.5">
                {list.map(t => <TicketCard key={t.task_key} ticket={t} onDecide={decide} />)}
              </div>
            </section>
          )
        })
      )}
    </div>
  )
}

function TicketCard({ ticket, onDecide }: {
  ticket: TriageTicket
  onDecide: (k: string, d: Decision, url?: string) => void
}) {
  const isStale = ticket.bucket === 'looks_stale'
  const isThin = ticket.bucket === 'needs_detail'
  return (
    <div className="rounded-xl border p-3.5 transition-all"
      style={{ background: 'var(--surface)', borderColor: 'var(--rule)' }}>
      <div className="flex items-start gap-2.5">
        <ProviderGlyph provider={ticket.provider} size={18} />
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2 mb-1 flex-wrap">
            <TaskKey keyId={ticket.task_key} />
            <span className="text-[13px] font-medium truncate" style={{ color: 'var(--ink)' }}>{ticket.title}</span>
          </div>
          {/* Reason hints */}
          <div className="flex flex-wrap gap-1.5 mt-1.5">
            {ticket.reasons.map((r, i) => (
              <span key={i} className="text-[11px] px-1.5 py-0.5 rounded"
                style={{ background: 'var(--tint)', color: 'var(--ink-2)' }}>
                {r.hint}
              </span>
            ))}
          </div>
          {isThin && ticket.description_excerpt && (
            <p className="text-[11px] mt-2 italic line-clamp-2" style={{ color: 'var(--ink-4)' }}>
              “{ticket.description_excerpt}”
            </p>
          )}
        </div>
      </div>

      {/* Actions — tailored to the bucket */}
      <div className="flex items-center gap-2 mt-3 pl-[28px]">
        {isThin && ticket.url && (
          <ActionBtn primary onClick={() => onDecide(ticket.task_key, 'keep', ticket.url)}>
            Add detail ↗
          </ActionBtn>
        )}
        <ActionBtn onClick={() => onDecide(ticket.task_key, 'keep')}>Keep</ActionBtn>
        {(isStale || ticket.bucket === 'not_sure') && (
          <ActionBtn danger onClick={() => onDecide(ticket.task_key, 'excluded')}>Exclude</ActionBtn>
        )}
        <ActionBtn subtle onClick={() => onDecide(ticket.task_key, 'snoozed')}>Snooze</ActionBtn>
      </div>
    </div>
  )
}

function ActionBtn({ children, onClick, primary, danger, subtle }: {
  children: React.ReactNode
  onClick: () => void
  primary?: boolean
  danger?: boolean
  subtle?: boolean
}) {
  const style: React.CSSProperties = primary
    ? { background: 'var(--accent)', color: '#fff', borderColor: 'var(--accent)' }
    : danger
      ? { background: 'transparent', color: 'var(--warn)', borderColor: 'var(--rule)' }
      : subtle
        ? { background: 'transparent', color: 'var(--ink-4)', borderColor: 'transparent' }
        : { background: 'transparent', color: 'var(--ink-2)', borderColor: 'var(--rule)' }
  return (
    <button onClick={onClick}
      className="text-[12px] px-2.5 py-1 rounded-md border transition-colors"
      style={style}>
      {children}
    </button>
  )
}

function AllClear({ total, readyCount, touched }: { total: number; readyCount: number; touched: number }) {
  return (
    <div className="rounded-2xl border p-8 text-center"
      style={{ background: 'var(--surface)', borderColor: 'var(--rule)' }}>
      <div aria-hidden style={{ fontSize: 32 }} className="mb-2">✨</div>
      <h2 className="text-[18px] font-medium mb-1" style={{ color: 'var(--ink)' }}>Your board is clean</h2>
      <p className="text-sm" style={{ color: 'var(--ink-3)' }}>
        {touched > 0
          ? `You tidied ${touched} ticket${touched === 1 ? '' : 's'}. `
          : ''}
        {readyCount} of {total} tickets are ready for accurate auto-worklogs. Meridian takes it from here.
      </p>
    </div>
  )
}
