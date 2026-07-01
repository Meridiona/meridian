//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Reusable ticket-detail dialog for the timeline app — full description +
// acceptance criteria (get_task_detail), themed with the mt-* timeline
// tokens. Any surface that shows a ticket key/title — Today's focus, the
// Tasks modal, the daily-plan modal, timeline cards, worklog rows — opens
// one by rendering <TaskDetailDialog taskKey={...} onClose={...} /> at the
// shell's modal layer; it owns its own fetch, loading state, and
// Escape/backdrop close. `inToday`/`canEdit`/`onAdd`/`onRemove` are optional
// — only the daily-plan caller wires them, everyone else gets a read-only dialog.

'use client'

import { useEffect, useState } from 'react'
import type { TaskDetail } from '@/lib/api-types'
import { load } from '@/lib/bridge'

export function TaskDetailDialog({
  taskKey, fallbackTitle, onClose, inToday, canEdit = true, onAdd, onRemove,
}: {
  taskKey: string
  fallbackTitle?: string
  onClose: () => void
  // Today's-plan add/remove — only rendered when the caller wires them (the
  // read-only openers on the timeline/tasks surfaces omit all four).
  inToday?: boolean
  canEdit?: boolean
  onAdd?: () => void
  onRemove?: () => void
}) {
  const [detail, setDetail] = useState<TaskDetail | null>(null)
  const [loading, setLoading] = useState(true)

  useEffect(() => {
    function onKey(e: KeyboardEvent) { if (e.key === 'Escape') onClose() }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose])

  useEffect(() => {
    let alive = true
    setLoading(true)
    load<TaskDetail | null>('/api/plan/task', 'get_task_detail', { key: taskKey })
      .then(d => { if (alive) { setDetail(d); setLoading(false) } })
      .catch(() => { if (alive) setLoading(false) })
    return () => { alive = false }
  }, [taskKey])

  const title = detail?.title ?? fallbackTitle ?? taskKey
  const dueLabel = detail?.due_days == null ? null
    : detail.due_days < 0 ? `overdue ${-detail.due_days}d`
      : detail.due_days === 0 ? 'today' : detail.due_days === 1 ? 'tomorrow' : `in ${detail.due_days}d`
  const hasMeta = !!(detail?.epic || detail?.priority || detail?.story_points || detail?.due_date)

  return (
    <div className="absolute inset-0 z-50 flex items-start justify-center p-6 sm:p-10 rise"
      style={{ background: 'rgba(20,16,40,0.5)', backdropFilter: 'blur(3px)' }} onClick={onClose}>
      <div className="w-full rounded-2xl overflow-hidden flex flex-col bg-panel"
        style={{ maxWidth: 640, maxHeight: '88%', border: '1px solid var(--t-card-border)', boxShadow: '0 24px 60px -18px rgba(20,16,40,0.5)' }}
        onClick={e => e.stopPropagation()}>
        <div className="px-6 pt-5 pb-4 border-b shrink-0" style={{ borderColor: 'var(--t-hair)' }}>
          <div className="flex items-center gap-2 mb-2.5">
            <span className="mt-mono-sm text-[11px] px-1.5 py-0.5 rounded bg-key-bg text-key-text">{taskKey}</span>
            {detail?.issue_type && (
              <span className="mt-chip px-1.5 py-0.5 rounded" style={{ color: 'var(--t-muted)', border: '1px solid var(--t-hair)' }}>
                {detail.issue_type}
              </span>
            )}
            {detail?.status && (
              <span className="mt-body-sm inline-flex items-center gap-1.5" style={{ color: 'var(--t-muted)' }}>
                <span className="inline-block w-1.5 h-1.5 rounded-full"
                  style={{ background: detail.is_terminal ? 'var(--color-state-approved)' : 'var(--color-state-proposal)' }} />
                {detail.status}
              </span>
            )}
            <button onClick={onClose} aria-label="Close"
              className="ml-auto inline-flex items-center justify-center rounded-full bg-wrap shrink-0"
              style={{ width: 28, height: 28, color: 'var(--t-muted)' }}>
              <span className="text-[16px] leading-none">×</span>
            </button>
          </div>
          <p className="mt-modal-title text-title">{title}</p>
          {hasMeta && (
            <div className="flex flex-wrap items-center gap-x-4 gap-y-1 mt-2.5">
              {detail?.epic && <MetaBit label="Epic" value={detail.epic} />}
              {detail?.priority && <MetaBit label="Priority" value={detail.priority} />}
              {detail?.story_points && <MetaBit label="Points" value={detail.story_points} />}
              {detail?.due_date && (
                <MetaBit label="Due" value={dueLabel ?? detail.due_date} warn={detail.due_days !== null && (detail.due_days as number) <= 1} />
              )}
            </div>
          )}
        </div>

        <div className="overflow-y-auto nice-scroll p-6 space-y-5">
          {loading && !detail ? (
            <p className="mt-body-sm italic" style={{ color: 'var(--t-faint-2)' }}>Loading…</p>
          ) : (
            <>
              <div>
                <p className="mt-label mb-2" style={{ color: 'var(--t-faint)' }}>Description</p>
                {detail?.description?.trim() ? (
                  <p className="mt-body whitespace-pre-wrap" style={{ color: 'var(--t-muted)' }}>{detail.description}</p>
                ) : (
                  <p className="mt-body-sm italic" style={{ color: 'var(--t-faint-2)' }}>No description on this ticket.</p>
                )}
              </div>
              {detail?.acceptance_criteria && (
                <div>
                  <p className="mt-label mb-2" style={{ color: 'var(--t-faint)' }}>Acceptance criteria</p>
                  <p className="mt-body whitespace-pre-wrap" style={{ color: 'var(--t-muted)' }}>{detail.acceptance_criteria}</p>
                </div>
              )}
            </>
          )}
        </div>

        {(detail?.url || onAdd || onRemove) && (
          <div className="px-6 py-4 border-t shrink-0 flex items-center gap-2.5" style={{ borderColor: 'var(--t-hair)' }}>
            {canEdit && (inToday ? (onRemove && (
              <button onClick={() => { onRemove(); onClose() }}
                className="mt-body-sm px-3.5 py-2 rounded-lg bg-ctrl"
                style={{ border: '1px solid var(--t-ctrl-border)', color: 'var(--t-muted)' }}>
                Remove from today
              </button>
            )) : (onAdd && (
              <button onClick={() => { onAdd(); onClose() }}
                className="mt-body-sm px-3.5 py-2 rounded-lg"
                style={{ background: 'var(--color-state-approved)', color: '#fff' }}>
                + Add to today
              </button>
            )))}
            {detail?.url && (
              <a href={detail.url} target="_blank" rel="noopener noreferrer"
                className="mt-body-sm px-3.5 py-2 rounded-lg inline-block bg-ctrl ml-auto"
                style={{ border: '1px solid var(--t-ctrl-border)', color: 'var(--t-muted)' }}>
                Open in tracker ↗
              </a>
            )}
          </div>
        )}
      </div>
    </div>
  )
}

function MetaBit({ label, value, warn = false }: { label: string; value: string; warn?: boolean }) {
  return (
    <span className="mt-body-sm" style={{ color: warn ? 'var(--color-state-pending)' : 'var(--t-muted)' }}>
      <span style={{ color: 'var(--t-faint)' }}>{label}: </span>{value}
    </span>
  )
}
