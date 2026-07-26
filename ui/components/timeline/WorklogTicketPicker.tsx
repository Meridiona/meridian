//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The human override for worklog matching.
//
// The matcher only ever compares a day's work against that day's PLANNED tasks —
// a far better prior than the whole board, but it means work on something you
// didn't plan can only ever come back as a proposal. This is how you say "no,
// file it against THAT one" over every open ticket you have — including your
// personal tasks (filed onto their own row, since there's no tracker thread to
// post to), which are badged so the list stays legible.
//
// Picking retargets, it does NOT regenerate: the written update describes the
// work, and the work doesn't change based on where it gets filed. So this
// resolves instantly instead of costing another minute of LLM time to rewrite
// prose that was already right.
//
// Deliberately uncapped. Matching against 50 tickets is a bad prompt; SEARCHING
// 50 tickets is a fine list — a person can type three letters where a model has
// to read all of them. That difference is why the old ">40 open tickets, refuse
// to generate" gate died with this component's arrival.

'use client'

import { useEffect, useMemo, useState } from 'react'
import { load } from '@/lib/bridge'
import type { BoardTicket } from '@/lib/api-types'

/** Rank the board against a query: key first (people type "KAN-3"), then title,
 *  then epic. Empty query keeps the server's newest-key-first order. */
function filterTickets(all: BoardTicket[], q: string): BoardTicket[] {
  const needle = q.trim().toLowerCase()
  if (!needle) return all
  return all.filter(t =>
    t.task_key.toLowerCase().includes(needle) ||
    t.title.toLowerCase().includes(needle) ||
    t.epic_title.toLowerCase().includes(needle),
  )
}

/** A searchable list of every open ticket, for retargeting a draft by hand.
 *  `current` is the draft's existing target, shown as already-selected so the
 *  user can see what they're changing away from. */
export function WorklogTicketPicker({ current, busy, onPick, onCancel, title, excludeLocal = false }: {
  current: string | null
  busy: boolean
  onPick: (taskKey: string) => void
  onCancel: () => void
  // Header copy - defaults to the retarget wording; escalation overrides it.
  title?: string
  // Drop personal (provider 'local') tasks from the list. Escalating a personal
  // task is onto a REAL tracker ticket, so its own kind must not appear.
  excludeLocal?: boolean
}) {
  const [all, setAll] = useState<BoardTicket[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [q, setQ] = useState('')

  useEffect(() => {
    let alive = true
    load<BoardTicket[]>('/api/board-tickets', 'get_board_tickets')
      .then(r => { if (alive) setAll(excludeLocal ? r.filter(t => t.provider !== 'local') : r) })
      .catch(() => { if (alive) setError('Could not load your board - try again in a moment.') })
    return () => { alive = false }
  }, [excludeLocal])

  const shown = useMemo(() => filterTickets(all ?? [], q), [all, q])

  return (
    <div className="space-y-2">
      <div className="flex items-center justify-between gap-2">
        <p className="mt-body-sm" style={{ color: 'var(--t-title)', fontSize: 12.5, fontWeight: 700 }}>
          {title ?? 'Pick the ticket this work belongs to'}
        </p>
        <button onClick={onCancel}
          className="mt-body-sm rounded-lg px-2 py-1"
          style={{ color: 'var(--t-muted)', fontSize: 12, cursor: 'pointer' }}>
          Cancel
        </button>
      </div>

      <input
        autoFocus
        value={q}
        onChange={e => setQ(e.target.value)}
        placeholder={all ? `Search ${all.length} open tickets…` : 'Search your board…'}
        className="w-full rounded-lg px-3 py-2"
        style={{
          fontSize: 12.5,
          color: 'var(--t-title)',
          background: 'var(--t-input)',
          border: '1px solid var(--t-input-border)',
          outline: 'none',
        }}
      />

      {error && (
        <p className="mt-body-sm rounded-lg px-3 py-2"
          style={{ color: 'var(--color-state-pending)', background: 'color-mix(in srgb, var(--color-state-pending) 12%, transparent)', fontSize: 12 }}>
          {error}
        </p>
      )}

      {!all && !error && (
        <p className="mt-body-sm px-1 py-2" style={{ color: 'var(--t-faint)', fontSize: 12 }}>Loading your board…</p>
      )}

      {all && shown.length === 0 && (
        <p className="mt-body-sm px-1 py-2" style={{ color: 'var(--t-faint)', fontSize: 12 }}>
          {all.length === 0
            ? 'No open tickets on your board. Meridian will propose a new one instead.'
            : `Nothing matches "${q.trim()}".`}
        </p>
      )}

      {shown.length > 0 && (
        <div className="nice-scroll space-y-1" style={{ maxHeight: 220, overflowY: 'auto' }}>
          {shown.map(t => {
            const isCurrent = t.task_key === current
            return (
              <button
                key={t.task_key}
                disabled={busy || isCurrent}
                onClick={() => onPick(t.task_key)}
                className="w-full text-left rounded-lg px-2.5 py-2"
                style={{
                  border: '1px solid var(--t-hair)',
                  background: isCurrent ? 'color-mix(in srgb, var(--color-state-proposal) 10%, transparent)' : 'transparent',
                  opacity: busy && !isCurrent ? 0.55 : 1,
                  cursor: busy || isCurrent ? 'default' : 'pointer',
                }}>
                <div className="flex items-center gap-1.5 min-w-0">
                  <span className="mt-mono-sm shrink-0 px-1.5 py-0.5 rounded bg-key-bg text-key-text" style={{ fontSize: 11 }}>
                    {t.task_key}
                  </span>
                  <span className="mt-body-sm truncate" style={{ color: 'var(--t-title)', fontSize: 12.5 }}>{t.title}</span>
                  <span className="flex items-center gap-1.5 shrink-0 ml-auto">
                    {t.provider === 'local' && (
                      <span className="mt-chip" style={{ color: 'var(--t-muted)', fontSize: 10.5 }}>Personal</span>
                    )}
                    {isCurrent && (
                      <span className="mt-chip" style={{ color: 'var(--color-state-proposal)', fontSize: 10.5 }}>Current</span>
                    )}
                  </span>
                </div>
                {t.epic_title && (
                  <p className="mt-body-sm mt-0.5 truncate" style={{ color: 'var(--t-faint)', fontSize: 11 }}>{t.epic_title}</p>
                )}
              </button>
            )
          })}
        </div>
      )}
    </div>
  )
}
