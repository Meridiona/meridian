//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// One worklog / proposed-ticket card on the one-pager timeline. Supersedes the
// old WorklogBlock (compact, in-timeline) AND the WorklogDetailPane action row
// (detail, in the hour panel), unified behind a `variant` prop. Card anatomy:
// a state-keyed left accent bar, a slim header (issue-type badge — no ticket
// key, minutes, status chip), title, summary (compact: first few words only;
// detail: full body + actions). Dismissed/rejected rows stay visible at half
// opacity rather than being filtered out.

'use client'

import { useState } from 'react'
import { fmtDur } from '@/components/atoms'
import { ProviderIcon } from '@/components/ProviderIcon'
import type { WorklogItem } from '@/lib/api-types'
import { EditableSummary } from './EditableSummary'
import { ReviewRejectPicker } from './ReviewRejectPicker'
import { stateColor, stateLabel, visualState, type RejectCorrection } from './types'
import type { WorklogActions } from './useTimelineData'

// Compact-card summary preview — just the first few words, not the full comment.
function firstWords(text: string, n = 10): string {
  const words = text.trim().split(/\s+/)
  return words.length <= n ? text.trim() : words.slice(0, n).join(' ') + '…'
}

export function TimelineCard({
  item, variant = 'compact', actions, selected = false,
}: {
  item: WorklogItem
  variant?: 'compact' | 'detail'
  actions?: WorklogActions
  // On the timeline, the specific card the user clicked — "pops" it forward
  // (lift + accent-colored border) instead of the whole hour row highlighting.
  selected?: boolean
}) {
  const accent = stateColor(item)
  const dimmed = visualState(item) === 'rejected'
  const minutes = fmtDur(item.time_spent_seconds)
  const detail = variant === 'detail'

  return (
    <div className={`rounded-xl overflow-hidden bg-card mt-card-hover ${selected ? 'mt-card-selected' : ''}`}
      style={{
        borderTop: `1px solid ${selected ? accent : 'var(--t-card-border)'}`,
        borderRight: `1px solid ${selected ? accent : 'var(--t-card-border)'}`,
        borderBottom: `1px solid ${selected ? accent : 'var(--t-card-border)'}`,
        borderLeft: `3px solid ${accent}`,
        opacity: dimmed ? 0.5 : 1,
      }}>
      <div className={detail ? 'p-5 space-y-3' : 'px-4 py-3.5 space-y-1.5'}>
        <div className="flex items-start gap-2">
          {item.task_title && (
            <div className="flex items-start gap-1.5 flex-1 min-w-0">
              {item.provider && <ProviderIcon provider={item.provider} size={13} className="shrink-0 mt-0.5" />}
              <p className={`mt-card-title text-title ${detail ? '' : 'truncate'}`}>{item.task_title}</p>
            </div>
          )}
          <div className="flex items-center gap-2 shrink-0">
            <span className="mt-mono-sm text-[11px]" style={{ color: 'var(--t-faint)' }}>{minutes}</span>
            <span className="mt-chip px-1.5 py-0.5 rounded" style={{ color: accent, border: `1px solid ${accent}` }}>
              {stateLabel(item)}
            </span>
          </div>
        </div>

        {detail ? (
          <DetailBody item={item} actions={actions} />
        ) : (
          item.summary && <p className="mt-body-sm truncate" style={{ color: 'var(--t-muted)' }}>{firstWords(item.summary)}</p>
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
    <div className="space-y-3">
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
        <div className="flex items-center gap-2.5 pt-1.5">
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
