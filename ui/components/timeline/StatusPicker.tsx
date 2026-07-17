//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// A dropdown that changes a task's status on its real tracker. The options are
// the provider's actual workflow statuses (Jira transitions, Linear states,
// Trello lists, Azure states, GitHub open/closed), loaded lazily each time the
// menu opens - Jira's reachable transitions depend on the current status, so a
// fresh fetch on open keeps the choices honest. The trigger reflects the task's
// CURRENT status from props (task.status/is_terminal) so it stays in sync with
// the parent's optimistic update after a pick. Picking calls onPick; the parent
// (TasksPanel) owns the write, the optimistic update, and the Undo snackbar.

'use client'

import { useEffect, useRef, useState } from 'react'
import { load } from '@/lib/bridge'
import type { TaskSummary, StatusListResponse, TaskStatusOption } from '@/lib/api-types'

// Canonical category → a dot colour, so a status reads as done / active / to-do
// at a glance regardless of provider.
function categoryColor(category: string, isTerminal: boolean): string {
  if (isTerminal || category === 'done') return 'var(--color-state-approved)'
  if (category === 'cancelled') return 'var(--t-faint)'
  if (category === 'in_progress' || category === 'in_review') return 'var(--color-state-proposal)'
  if (category === 'todo') return 'var(--color-state-pending)'
  return 'var(--t-faint)'
}

export function StatusPicker({ task, onPick, busy }: {
  task: TaskSummary
  onPick: (option: TaskStatusOption) => void
  busy: boolean
}) {
  const [open, setOpen] = useState(false)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [options, setOptions] = useState<TaskStatusOption[]>([])
  const ref = useRef<HTMLDivElement>(null)

  // Close on outside click / Escape.
  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent) => { if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false) }
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') setOpen(false) }
    document.addEventListener('mousedown', onDown)
    document.addEventListener('keydown', onKey)
    return () => { document.removeEventListener('mousedown', onDown); document.removeEventListener('keydown', onKey) }
  }, [open])

  const loadStatuses = () => {
    setLoading(true); setError(null)
    load<StatusListResponse>('/api/tasks/statuses', 'list_task_statuses', { provider: task.provider, key: task.key })
      .then(r => setOptions(r.statuses ?? []))
      .catch(e => setError(e instanceof Error ? e.message : typeof e === 'string' ? e : 'Could not load statuses'))
      .finally(() => setLoading(false))
  }

  const toggle = () => {
    const next = !open
    setOpen(next)
    if (next) loadStatuses() // fresh each open — Jira transitions are position-dependent
  }

  const dot = categoryColor('', task.is_terminal)
  // Hide the option that equals the current status (by name) — you don't
  // transition to where you already are.
  const currentName = (task.status || '').toLowerCase()

  return (
    <div className="relative inline-block" ref={ref}>
      <button
        onClick={toggle}
        disabled={busy}
        className="mt-body-sm inline-flex items-center gap-1.5 rounded-md px-2 py-1"
        style={{ border: '1px solid var(--t-hair)', background: 'var(--t-box)', color: 'var(--t-muted)', opacity: busy ? 0.6 : 1, cursor: busy ? 'default' : 'pointer' }}
        title="Change status on your tracker">
        <span className="inline-block w-1.5 h-1.5 rounded-full" style={{ background: dot }} />
        {task.status || '—'}
        <span style={{ fontSize: 9, opacity: 0.6 }}>{busy ? '⋯' : '▾'}</span>
      </button>

      {open && (
        <div className="absolute z-50 mt-1 rounded-xl overflow-hidden"
          style={{ minWidth: 200, maxHeight: 280, overflowY: 'auto', background: 'var(--t-card)', border: '1px solid var(--t-card-border)', boxShadow: '0 12px 34px -12px rgba(0,0,0,0.4)' }}>
          <div className="px-3 py-2" style={{ borderBottom: '1px solid var(--t-hair)' }}>
            <p className="mt-label" style={{ color: 'var(--t-faint)' }}>Move to</p>
          </div>
          {loading ? (
            <p className="mt-body-sm px-3 py-3" style={{ color: 'var(--t-faint)' }}>Loading…</p>
          ) : error ? (
            <p className="mt-body-sm px-3 py-3" style={{ color: 'var(--color-state-pending)' }}>{error}</p>
          ) : options.filter(o => o.name.toLowerCase() !== currentName).length === 0 ? (
            <p className="mt-body-sm px-3 py-3" style={{ color: 'var(--t-faint)' }}>No other statuses available.</p>
          ) : (
            options
              .filter(o => o.name.toLowerCase() !== currentName)
              .map(o => (
                <button key={o.id + o.name}
                  onClick={() => { setOpen(false); onPick(o) }}
                  className="w-full text-left px-3 py-2 flex items-center gap-2 mt-row-hover"
                  style={{ color: 'var(--t-title)' }}>
                  <span className="inline-block w-1.5 h-1.5 rounded-full shrink-0" style={{ background: categoryColor(o.category, false) }} />
                  <span className="mt-body-sm truncate">{o.name}</span>
                </button>
              ))
          )}
        </div>
      )}
    </div>
  )
}
