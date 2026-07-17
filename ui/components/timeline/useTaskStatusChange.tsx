//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Shared status-change logic for anywhere a `StatusPicker` is rendered
// (TasksPanel's detail pane, CleanupCard's board-cleanup queue): the mutate
// call, the optimistic patch, and the lingering Undo/redirect feedback are
// identical in both places, so they live here once rather than being
// duplicated per screen. The caller supplies only how to patch ITS OWN local
// task list — everything else (busy tracking, Undo capture, the transient
// note for a redirected/errored change) is owned by this hook.

'use client'

import { useRef, useState, useEffect } from 'react'
import type { TaskSummary, TaskStatusOption, SetStatusResponse } from '@/lib/api-types'
import { mutate, openExternal } from '@/lib/bridge'

// How long the Undo affordance (or a redirect/error note) lingers before it fades.
const UNDO_LINGER_MS = 10_000

export function isTerminalCategory(category: string): boolean {
  return category === 'done' || category === 'cancelled'
}

// What a status change captured, so Undo can put it back.
export interface UndoEntry {
  taskKey: string
  provider: string
  prevStatus: string   // the status name before the change (set accepts id-or-name)
  prevTerminal: boolean
  newName: string
}

/** Drives a `StatusPicker` against `/api/tasks/status` (`set_task_status`):
 *  tracks which task's write is in flight, captures an Undo entry on success,
 *  and surfaces a transient note when the tracker couldn't apply it in-app
 *  (redirected to the browser) or the call errored. `patchTaskStatus` is the
 *  ONLY thing each caller supplies — it applies the same `{status,
 *  is_terminal}` patch this hook always computes to whatever local list that
 *  caller renders from (TasksPanel's `data.tasks`, CleanupOverlay's `queue`).
 *  `onUndoSettled` is optional — TasksPanel uses it to re-sync from the
 *  server after an undo; a caller with no such refetch just omits it. */
export function useTaskStatusChange(
  patchTaskStatus: (key: string, status: string, isTerminal: boolean) => void,
  options?: { onUndoSettled?: () => void },
) {
  const [statusBusyKey, setStatusBusyKey] = useState<string | null>(null)
  const [undo, setUndo] = useState<UndoEntry | null>(null)
  const [note, setNote] = useState<string | null>(null)
  const undoTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const noteTimer = useRef<ReturnType<typeof setTimeout> | null>(null)

  useEffect(() => () => {
    if (undoTimer.current) clearTimeout(undoTimer.current)
    if (noteTimer.current) clearTimeout(noteTimer.current)
  }, [])

  const flashNote = (msg: string) => {
    setNote(msg)
    if (noteTimer.current) clearTimeout(noteTimer.current)
    noteTimer.current = setTimeout(() => setNote(null), UNDO_LINGER_MS)
  }

  const dismissUndo = () => {
    if (undoTimer.current) clearTimeout(undoTimer.current)
    setUndo(null)
  }

  const dismissNote = () => setNote(null)

  // Change a task's status on its tracker, capturing an Undo.
  const handleSetStatus = (task: TaskSummary, option: TaskStatusOption) => {
    if (statusBusyKey) return
    const prevStatus = task.status
    const prevTerminal = task.is_terminal
    setStatusBusyKey(task.key)
    setNote(null)
    mutate<SetStatusResponse>('/api/tasks/status', 'set_task_status',
      { provider: task.provider, key: task.key, status_id: option.id })
      .then(res => {
        if (res.result.status === 'applied') {
          const ns = res.new_status
          const name = ns?.name ?? option.name
          const terminal = ns ? isTerminalCategory(ns.category) : isTerminalCategory(option.category)
          patchTaskStatus(task.key, name, terminal)
          if (undoTimer.current) clearTimeout(undoTimer.current)
          setUndo({ taskKey: task.key, provider: task.provider, prevStatus, prevTerminal, newName: name })
          undoTimer.current = setTimeout(() => setUndo(null), UNDO_LINGER_MS)
        } else {
          // Tracker couldn't do it in-app — the browser was opened to finish.
          if (res.result.browse_url) openExternal(res.result.browse_url)
          flashNote(res.result.reason || `Finish the change in ${task.provider} - opened in your browser.`)
        }
      })
      .catch(e => flashNote(e instanceof Error ? e.message : typeof e === 'string' ? e : 'Could not change status'))
      .finally(() => setStatusBusyKey(null))
  }

  // Put the status back to what it was before the last change.
  const handleUndo = () => {
    const u = undo
    if (!u || statusBusyKey) return
    dismissUndo()
    setStatusBusyKey(u.taskKey)
    // set accepts id-or-name; the previous status NAME is what we captured.
    mutate<SetStatusResponse>('/api/tasks/status', 'set_task_status',
      { provider: u.provider, key: u.taskKey, status_id: u.prevStatus })
      .then(res => {
        if (res.result.status === 'applied') {
          patchTaskStatus(u.taskKey, u.prevStatus, u.prevTerminal)
        } else {
          if (res.result.browse_url) openExternal(res.result.browse_url)
          flashNote(res.result.reason || `Finish the undo in ${u.provider} - opened in your browser.`)
        }
      })
      .catch(e => flashNote(e instanceof Error ? e.message : typeof e === 'string' ? e : 'Could not undo'))
      .finally(() => { setStatusBusyKey(null); options?.onUndoSettled?.() })
  }

  return { statusBusyKey, undo, note, handleSetStatus, handleUndo, dismissUndo, dismissNote }
}

// Lingering status-change feedback: an Undo bar (10s) after a successful change,
// or a transient note when the change was redirected to the tracker / errored.
// Fixed to the viewport bottom-center, so it reads the same whether the caller
// is a plain panel (TasksPanel) or an already-fixed full-screen overlay
// (CleanupOverlay) — both just mount it once at their root.
export function StatusBanner({ undo, note, busy, onUndo, onDismissUndo, onDismissNote }: {
  undo: UndoEntry | null
  note: string | null
  busy: boolean
  onUndo: () => void
  onDismissUndo: () => void
  onDismissNote: () => void
}) {
  if (!undo && !note) return null
  return (
    <div className="fixed left-1/2 -translate-x-1/2 z-[60]" style={{ bottom: 24 }}>
      {undo ? (
        <div className="flex items-center gap-3 rounded-xl px-4 py-2.5"
          style={{ background: 'var(--t-card)', border: '1px solid var(--t-card-border)', boxShadow: '0 14px 40px -14px rgba(0,0,0,0.5)' }}>
          <span className="mt-body-sm" style={{ color: 'var(--t-title)' }}>
            <span className="mt-mono-sm text-[11px] px-1.5 py-0.5 rounded bg-key-bg text-key-text mr-1.5">{undo.taskKey}</span>
            moved to <span style={{ fontWeight: 700 }}>{undo.newName}</span>
          </span>
          <button onClick={onUndo} disabled={busy}
            className="mt-body-sm px-2.5 py-1 rounded-md"
            style={{ color: 'var(--color-state-proposal)', fontWeight: 700, border: '1px solid color-mix(in srgb, var(--color-state-proposal) 40%, transparent)', opacity: busy ? 0.6 : 1 }}>
            {busy ? 'Undoing…' : 'Undo'}
          </button>
          <button onClick={onDismissUndo} aria-label="Dismiss" className="mt-body-sm" style={{ color: 'var(--t-faint)', padding: '0 2px' }}>✕</button>
        </div>
      ) : (
        <div className="flex items-center gap-3 rounded-xl px-4 py-2.5"
          style={{ background: 'var(--t-card)', border: '1px solid color-mix(in srgb, var(--color-state-pending) 35%, var(--t-card-border))', boxShadow: '0 14px 40px -14px rgba(0,0,0,0.5)' }}>
          <span className="mt-body-sm" style={{ color: 'var(--t-muted)' }}>{note}</span>
          <button onClick={onDismissNote} aria-label="Dismiss" className="mt-body-sm" style={{ color: 'var(--t-faint)', padding: '0 2px' }}>✕</button>
        </div>
      )}
    </div>
  )
}
