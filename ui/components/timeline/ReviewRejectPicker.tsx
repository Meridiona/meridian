//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The "where should this time have gone?" attribution picker shown when
// dismissing a real worklog. Extracted from the old inline WorklogCard reject
// flow so both WorklogDetailPane's Dismiss button and ReviewOverlay's
// swipe-left decline step share one implementation. Proposed tickets don't use
// this — they have no "where should this have gone" concept, so their dismiss
// is a direct action with no picker (matches the pre-redesign ProposedCard).

'use client'

import { useEffect, useState } from 'react'
import { fetchRejectCandidates, type Candidate } from './useTimelineData'
import type { RejectCorrection } from './types'

export function ReviewRejectPicker({
  worklogId, excludeKey, busy, onConfirm, onCancel,
}: {
  worklogId: number
  excludeKey: string
  busy: boolean
  onConfirm: (correction: RejectCorrection) => void
  onCancel: () => void
}) {
  const [candidates, setCandidates] = useState<Candidate[] | null>(null)
  const [target, setTarget] = useState<string>('__unknown__')

  useEffect(() => {
    let alive = true
    fetchRejectCandidates(excludeKey)
      .then(c => { if (alive) setCandidates(c) })
      .catch(() => { if (alive) setCandidates([]) })
    return () => { alive = false }
  }, [excludeKey])

  function confirm() {
    const correction: RejectCorrection =
      target === '__untracked__' ? { correctedToUntracked: true }
        : target === '__unknown__' ? {}
          : { correctedTaskKey: target }
    onConfirm(correction)
  }

  return (
    <div className="rounded-2xl p-4 bg-card" style={{ border: '1px solid var(--t-card-border)' }}>
      <p className="mt-body mb-2" style={{ color: 'var(--t-title)' }}>
        Where should this time have gone? <span style={{ color: 'var(--t-faint)' }}>(helps Meridian learn)</span>
      </p>
      <div className="space-y-1 max-h-48 overflow-y-auto">
        {candidates == null ? (
          <p className="mt-body-sm" style={{ color: 'var(--t-muted)' }}>Loading tickets…</p>
        ) : (
          <>
            {candidates.map(c => (
              <label key={c.key} className="flex items-center gap-2 mt-body-sm cursor-pointer py-0.5" style={{ color: 'var(--t-title)' }}>
                <input type="radio" name={`reject-${worklogId}`} checked={target === c.key} onChange={() => setTarget(c.key)} />
                <span className="mt-mono-sm">{c.key}</span>
                <span className="truncate" style={{ color: 'var(--t-muted)' }}>{c.title}</span>
              </label>
            ))}
            <label className="flex items-center gap-2 mt-body-sm cursor-pointer py-0.5" style={{ color: 'var(--t-title)' }}>
              <input type="radio" name={`reject-${worklogId}`} checked={target === '__untracked__'} onChange={() => setTarget('__untracked__')} />
              Untracked / personal
            </label>
            <label className="flex items-center gap-2 mt-body-sm cursor-pointer py-0.5" style={{ color: 'var(--t-muted)' }}>
              <input type="radio" name={`reject-${worklogId}`} checked={target === '__unknown__'} onChange={() => setTarget('__unknown__')} />
              Just dismiss — not sure
            </label>
          </>
        )}
      </div>
      <div className="flex items-center gap-2 mt-3">
        <button onClick={confirm} disabled={busy}
          className="mt-body-sm px-3 py-1 rounded-md" style={{ background: 'var(--t-title)', color: 'var(--t-panel)' }}>
          Dismiss worklog
        </button>
        <button onClick={onCancel} disabled={busy}
          className="mt-body-sm px-3 py-1 rounded-md" style={{ color: 'var(--t-muted)', border: '1px solid var(--t-hair)' }}>
          Cancel
        </button>
      </div>
    </div>
  )
}
