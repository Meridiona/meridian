//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// "Match to a different ticket" picker — shown inline when editing a real
// (non-proposed) worklog card in Review Drafts. Sibling to
// ReviewRejectPicker (same candidate-fetch + radio-list shape) but simpler:
// there's no "untracked"/"just dismiss" option here, since re-matching keeps
// the drafted worklog alive against the new ticket rather than dismissing it
// — see meridian_core::worklogs::rematch_worklog for why that's a distinct
// action from reject's correctedTaskKey.
//
// Click-to-stage, not select-then-confirm: picking a candidate here is a
// PURELY LOCAL selection — no network call. ReviewCard stages it as
// `pendingCandidate` and shows it as the card's displayed key/title
// immediately, but nothing is written to the DB until the card's one Save
// button is clicked (which commits the summary text AND this pending
// ticket change together — see ReviewCard's `handleSave`). This mirrors how
// the summary textarea already works (typing doesn't save either) so there
// is exactly one commit point per edit session, not two.

'use client'

import { useEffect, useState } from 'react'
import { fetchRejectCandidates, type Candidate } from './useTimelineData'

export function TicketMatchPicker({
  currentKey, busy, onConfirm, onCancel,
}: {
  currentKey: string
  busy: boolean
  onConfirm: (candidate: Candidate) => void
  onCancel: () => void
}) {
  const [candidates, setCandidates] = useState<Candidate[] | null>(null)

  useEffect(() => {
    let alive = true
    fetchRejectCandidates(currentKey)
      .then(c => { if (alive) setCandidates(c) })
      .catch(() => { if (alive) setCandidates([]) })
    return () => { alive = false }
  }, [currentKey])

  return (
    <div className="rounded-md p-3 bg-box" style={{ border: '1px solid var(--t-hair)' }}>
      <div className="flex items-center justify-between mb-2">
        <p className="mt-label" style={{ color: 'var(--t-faint)' }}>Match to a different ticket</p>
        <button onClick={onCancel} disabled={busy} className="mt-body-sm" style={{ color: 'var(--t-muted)' }}>
          Cancel
        </button>
      </div>
      <div className="space-y-0.5 max-h-40 overflow-y-auto">
        {candidates == null ? (
          <p className="mt-body-sm" style={{ color: 'var(--t-muted)' }}>Loading tickets…</p>
        ) : candidates.length === 0 ? (
          <p className="mt-body-sm italic" style={{ color: 'var(--t-faint-2)' }}>No other tickets to match.</p>
        ) : (
          candidates.map(c => (
            <button key={c.key} onClick={() => onConfirm(c)} disabled={busy}
              className="mt-row-hover w-full flex items-center gap-2 mt-body-sm py-1.5 px-2 rounded-md text-left"
              style={{ opacity: busy ? 0.5 : 1 }}>
              <span className="mt-mono-sm text-[11px] shrink-0 px-1.5 py-0.5 rounded bg-key-bg text-key-text">{c.key}</span>
              <span className="truncate flex-1 min-w-0" style={{ color: 'var(--t-muted)' }}>{c.title}</span>
            </button>
          ))
        )}
      </div>
    </div>
  )
}
