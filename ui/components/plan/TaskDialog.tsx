//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

import { useEffect, useState } from 'react'
import { TaskKey, ProviderGlyph, StatusPill } from '@/components/atoms'
import { MetaChip } from '@/components/plan/parts'
import type { CardTask } from '@/components/plan/TaskCard'
import type { TaskDetail } from '@/lib/api-types'
import { load, openExternal } from '@/lib/bridge'

// Full-ticket dialog opened from a plan card. Shows the complete description and
// acceptance criteria (the list only carries an excerpt) and lets the dev add the
// task to / remove it from today, or open it in the tracker.
export default function TaskDialog({
  task, inToday, canEdit = true, onClose, onAdd, onRemove,
}: {
  task: CardTask
  inToday: boolean
  canEdit?: boolean
  onClose: () => void
  onAdd: () => void
  onRemove: () => void
}) {
  const [detail, setDetail] = useState<TaskDetail | null>(null)
  const [loading, setLoading] = useState(true)

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') onClose() }
    window.addEventListener('keydown', onKey)
    // Lock background scroll while the dialog is open.
    const prevOverflow = document.body.style.overflow
    document.body.style.overflow = 'hidden'
    return () => {
      window.removeEventListener('keydown', onKey)
      document.body.style.overflow = prevOverflow
    }
  }, [onClose])

  useEffect(() => {
    let alive = true
    setLoading(true)
    // Dual-path: get_task_detail (Rust) in the app, /api/plan/task in a browser.
    load<TaskDetail | null>('/api/plan/task', 'get_task_detail', { key: task.key })
      .then((d) => { if (alive) { setDetail(d); setLoading(false) } })
      .catch(() => { if (alive) setLoading(false) })
    return () => { alive = false }
  }, [task.key])

  // Fall back to the card's data while the full detail loads.
  const d = detail
  const title = d?.title ?? task.title
  const status = d?.status ?? task.status ?? ''
  const isTerminal = d?.is_terminal ?? task.is_terminal ?? false
  const epic = d?.epic ?? task.epic ?? null
  const priority = d?.priority ?? task.priority ?? null
  const points = d?.story_points ?? task.story_points ?? null
  const issueType = d?.issue_type ?? task.issue_type ?? ''
  const dueDays = d?.due_days ?? task.due_days ?? null
  const dueDate = d?.due_date ?? null
  const startDate = d?.start_date ?? null
  const url = d?.url || task.url
  const description = d?.description ?? task.description ?? ''
  const acceptance = d?.acceptance_criteria ?? null

  const dueLabel = dueDays === null ? null
    : dueDays < 0 ? `overdue ${-dueDays}d`
      : dueDays === 0 ? 'today' : dueDays === 1 ? 'tomorrow' : `in ${dueDays}d`

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4 rise"
      style={{ background: 'rgba(20,16,10,0.45)', backdropFilter: 'blur(3px)' }} onClick={onClose}>
      <div className="w-full max-w-2xl rounded-[20px] overflow-hidden flex flex-col max-h-[88vh]"
        style={{ background: 'var(--paper)', border: '1px solid var(--rule)', boxShadow: '0 24px 60px -12px rgba(20,16,10,0.35)' }}
        onClick={e => e.stopPropagation()}>
        <div style={{ height: 3, background: 'linear-gradient(90deg, var(--accent), #60A5FA)' }} />

        {/* Header */}
        <div className="px-7 pt-6 pb-5" style={{ background: 'var(--tint)', borderBottom: '1px solid var(--rule)' }}>
          <div className="flex items-center gap-2.5 mb-3">
            <ProviderGlyph provider={task.provider} size={18} />
            <TaskKey keyId={task.key} />
            <StatusPill status={status} isTerminal={isTerminal} />
            {issueType && !/^task$/i.test(issueType) && <MetaChip>{issueType}</MetaChip>}
            <button onClick={onClose} aria-label="Close"
              className="ml-auto inline-flex items-center justify-center rounded-full transition-colors"
              style={{ width: 28, height: 28, color: 'var(--ink-3)', background: 'var(--surface)', border: '1px solid var(--rule)' }}>
              <span className="text-[16px] leading-none">×</span>
            </button>
          </div>
          <h2 className="type-heading leading-snug" style={{ color: 'var(--ink)' }}>{title}</h2>
          <dl className="mt-4 grid grid-cols-[auto_1fr] gap-x-5 gap-y-2 text-[12px]">
            {epic && <MetaRow label="Epic"><span style={{ color: 'var(--ink-2)' }}>{epic}</span></MetaRow>}
            {dueDate && (
              <MetaRow label="Due">
                <span style={{ color: dueDays !== null && dueDays <= 1 ? 'var(--warn)' : 'var(--ink-2)' }}>
                  {fmtDate(dueDate)}{dueLabel ? ` · ${dueLabel}` : ''}
                </span>
              </MetaRow>
            )}
            {startDate && <MetaRow label="Start"><span style={{ color: 'var(--ink-2)' }}>{fmtDate(startDate)}</span></MetaRow>}
            {priority && <MetaRow label="Priority"><span style={{ color: 'var(--ink-2)' }}>{priority}</span></MetaRow>}
            {points && <MetaRow label="Points"><span style={{ color: 'var(--ink-2)' }}>{points}</span></MetaRow>}
          </dl>
        </div>

        {/* Body */}
        <div className="px-7 py-6 overflow-auto" style={{ background: 'var(--paper)' }}>
          {loading && !d ? (
            <p className="text-[13px]" style={{ color: 'var(--ink-3)' }}>Loading…</p>
          ) : (
            <>
              <Section label="Description">
                {description.trim()
                  ? <p className="text-[13px] leading-relaxed whitespace-pre-wrap" style={{ color: 'var(--ink-2)' }}>{description}</p>
                  : <p className="text-[13px] italic" style={{ color: 'var(--ink-4)' }}>No description on this ticket.</p>}
              </Section>
              {acceptance && (
                <Section label="Acceptance criteria">
                  <p className="text-[13px] leading-relaxed whitespace-pre-wrap" style={{ color: 'var(--ink-2)' }}>{acceptance}</p>
                </Section>
              )}
            </>
          )}
        </div>

        {/* Footer actions */}
        <div className="px-7 py-4 flex items-center gap-3" style={{ borderTop: '1px solid var(--rule)', background: 'var(--surface)' }}>
          {canEdit ? (inToday ? (
            <button onClick={() => { onRemove(); onClose() }}
              className="text-[13px] px-4 py-2 rounded-lg border font-medium transition-colors"
              style={{ borderColor: 'var(--rule)', color: 'var(--ink-2)', background: 'var(--paper)' }}>
              Remove from today
            </button>
          ) : (
            <button onClick={() => { onAdd(); onClose() }}
              className="text-[13px] px-4 py-2 rounded-lg font-medium transition-colors"
              style={{ background: 'var(--ink)', color: 'var(--paper)' }}>
              + Add to today
            </button>
          )) : (
            inToday && <span className="text-[12px]" style={{ color: 'var(--ink-3)' }}>In today’s plan</span>
          )}
          {url && (
            <a href={url} onClick={(e) => { e.preventDefault(); openExternal(url) }}
              className="ml-auto text-[12px] px-3.5 py-2 rounded-lg border transition-colors"
              style={{ borderColor: 'var(--rule)', color: 'var(--ink-2)', background: 'var(--paper)' }}>
              Open in tracker ↗
            </a>
          )}
        </div>
      </div>
    </div>
  )
}

function Section({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="mb-5 last:mb-0">
      <p className="text-[10px] uppercase tracking-[0.16em] mb-2" style={{ color: 'var(--ink-3)' }}>{label}</p>
      {children}
    </div>
  )
}

// One label/value pair in the header definition list. dt/dd are direct grid items.
function MetaRow({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <>
      <dt className="text-[10px] uppercase tracking-[0.12em] pt-0.5 whitespace-nowrap" style={{ color: 'var(--ink-4)' }}>{label}</dt>
      <dd className="min-w-0">{children}</dd>
    </>
  )
}

/** Format a due/start value (date-only or full timestamp) as "Jun 26". */
function fmtDate(s: string): string {
  const d = new Date(s.length <= 10 ? `${s}T00:00:00` : s)
  if (isNaN(d.getTime())) return s
  return d.toLocaleDateString('en-US', { month: 'short', day: 'numeric' })
}
