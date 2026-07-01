//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// One worklog / proposed-ticket card on the one-pager timeline. Supersedes the
// old WorklogBlock (compact, in-timeline) AND the WorklogDetailPane action row
// (detail, in the hour panel), unified behind a `variant` prop. Card anatomy
// follows the mock: a state-keyed left accent bar, a mono ticket-key badge +
// kind label + minutes + status chip header, title, summary. Dismissed/rejected
// rows stay visible at half opacity rather than being filtered out.

'use client'

import { useState } from 'react'
import { fmtDur } from '@/components/atoms'
import type { WorklogItem } from '@/lib/api-types'
import { EditableSummary } from './EditableSummary'
import { ReviewRejectPicker } from './ReviewRejectPicker'
import { kindLabel, stateColor, stateLabel, visualState, type RejectCorrection } from './types'
import type { WorklogActions } from './useTimelineData'

export function TimelineCard({
  item, variant = 'compact', actions,
}: {
  item: WorklogItem
  variant?: 'compact' | 'detail'
  actions?: WorklogActions
}) {
  const accent = stateColor(item)
  const dimmed = visualState(item) === 'rejected'
  const minutes = fmtDur(item.time_spent_seconds)
  const detail = variant === 'detail'

  return (
    <div className="rounded-xl overflow-hidden bg-card"
      style={{
        border: '1px solid var(--t-card-border)',
        borderLeft: `3px solid ${accent}`,
        opacity: dimmed ? 0.5 : 1,
      }}>
      <div className={detail ? 'p-4 space-y-2.5' : 'px-3.5 py-3 space-y-1.5'}>
        <div className="flex items-center gap-2">
          <span className="mt-mono-sm text-[11px] px-1.5 py-0.5 rounded bg-key-bg text-key-text">{item.task_key}</span>
          <span className="mt-body-sm truncate" style={{ color: 'var(--t-muted)' }}>{kindLabel(item)}</span>
          <span className="mt-mono-sm text-[11px] ml-auto" style={{ color: 'var(--t-faint)' }}>{minutes}</span>
          <span className="mt-chip px-1.5 py-0.5 rounded shrink-0" style={{ color: accent, border: `1px solid ${accent}` }}>
            {stateLabel(item)}
          </span>
        </div>

        {item.task_title && (
          <p className={`mt-card-title text-title ${detail ? '' : 'truncate'}`}>{item.task_title}</p>
        )}

        {detail ? (
          <DetailBody item={item} actions={actions} />
        ) : (
          item.summary && <p className="mt-body-sm truncate" style={{ color: 'var(--t-muted)' }}>{item.summary}</p>
        )}
      </div>
    </div>
  )
}

// The detail variant carries the summary + inline Dismiss/Edit/Approve actions
// that the old WorklogDetailPane exposed. Proposed rows route through the
// proposed-* mutations; real worklogs through act/reject/saveEdit.
function DetailBody({ item, actions }: { item: WorklogItem; actions?: WorklogActions }) {
  const [editing, setEditing] = useState(false)
  const [rejecting, setRejecting] = useState(false)
  const busy = actions?.busy === (item.is_proposed ? `prop:${item.id}` : `wl:${item.id}`)
  const posted = item.state === 'posted'
  const awaitingTicket = item.is_proposed && item.state === 'approved'

  const save = (s: string) => {
    if (item.is_proposed) actions?.saveProposedBody(item.id, s)
    else actions?.saveEdit(item.id, s)
    setEditing(false)
  }

  return (
    <div className="space-y-2.5">
      {editing ? (
        <EditableSummary label="Summary" value={item.summary}
          placeholder="(empty — add a comment)" busy={!!busy} rows={3} onSave={save} />
      ) : (
        <p className="mt-body whitespace-pre-wrap" style={{ color: item.summary ? 'var(--t-title)' : 'var(--t-faint)' }}>
          {item.summary || '(empty — nothing to post)'}
        </p>
      )}

      {item.reasoning && !editing && (
        <div className="rounded-md p-2.5 bg-box">
          <p className="mt-label mb-1" style={{ color: 'var(--t-faint)' }}>
            {item.is_proposed ? 'Why a new ticket' : 'Why this task'}
          </p>
          <p className="mt-body-sm" style={{ color: 'var(--t-muted)' }}>{item.reasoning}</p>
        </div>
      )}

      {rejecting ? (
        <ReviewRejectPicker worklogId={item.id} excludeKey={item.task_key} busy={!!busy}
          onConfirm={(c: RejectCorrection) => { actions?.reject(item.id, c); setRejecting(false) }}
          onCancel={() => setRejecting(false)} />
      ) : awaitingTicket ? (
        <p className="mt-body-sm" style={{ color: 'var(--t-muted)' }}>
          ✓ Approved — waiting for the daemon to create the ticket and post this worklog.
        </p>
      ) : !posted && !editing && (
        <div className="flex items-center gap-2 pt-1">
          <button onClick={() => item.is_proposed ? actions?.proposedAct(item.id, 'approve') : actions?.act(item.id, 'approve')}
            disabled={busy || (item.is_proposed ? !item.task_title?.trim() : !item.summary.trim())}
            className="mt-body-sm px-3 py-1.5 rounded-md"
            style={{ background: 'var(--color-state-approved)', color: '#fff', opacity: busy ? 0.6 : 1 }}>
            Approve ✓
          </button>
          <button onClick={() => setEditing(true)} disabled={busy}
            className="mt-body-sm px-3 py-1.5 rounded-md" style={{ color: 'var(--t-muted)', border: '1px solid var(--t-hair)' }}>
            Edit ✎
          </button>
          <button onClick={() => item.is_proposed ? actions?.proposedAct(item.id, 'dismiss') : setRejecting(true)}
            disabled={busy} className="mt-body-sm px-3 py-1.5 rounded-md ml-auto" style={{ color: 'var(--t-faint)' }}>
            Dismiss ✕
          </button>
        </div>
      )}

      {editing && (
        <button onClick={() => setEditing(false)} className="mt-body-sm px-3 py-1 rounded-md"
          style={{ color: 'var(--t-muted)', border: '1px solid var(--t-hair)' }}>Cancel edit</button>
      )}
    </div>
  )
}
