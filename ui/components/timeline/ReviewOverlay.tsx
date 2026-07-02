//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The "Review pending" swipe dialog — a queue of drafted worklogs + proposed
// tickets presented one at a time. Owns its own queue snapshot + index so an
// in-flight review isn't reshuffled by useTimelineData's 30s poll; every action
// flows through the shared mutation callbacks, so the timeline updates
// automatically. Self-contained backdrop (ReviewModal just mounts this).
//
// `focusKey` scopes the queue to a single ticket — clicking a draft card on
// the timeline board opens straight into review for THAT card (rather than
// the right-side Hour-detail panel, which is reserved for already-approved
// work — see TimelineColumn/MeridianTimelineShell), instead of dropping the
// user into the front of the whole pending queue. Editing an approved/posted
// card (via the right panel's own "Edit" action) reuses the same `focusKey`
// path — since there's nothing left to swipe-approve/dismiss on a card
// that's already been decided, that case opens straight into edit mode and
// Save/Cancel close the dialog outright instead of falling back to the
// swipe FABs (see `editOnly` below).

'use client'

import { useEffect, useMemo, useState } from 'react'
import { AnimatePresence } from 'framer-motion'
import type { WorklogItem } from '@/lib/api-types'
import { ReviewCard, type ReviewDirection } from './ReviewCard'
import { ReviewRejectPicker } from './ReviewRejectPicker'
import { itemKey, isPending, type RejectCorrection } from './types'
import type { WorklogActions } from './useTimelineData'

export function ReviewOverlay({ items, actions, focusKey, onClose }: {
  items: WorklogItem[]
  actions: WorklogActions
  focusKey?: string | null
  onClose: () => void
}) {
  // Snapshot at open time — see file header for why (order/length only).
  const [queue] = useState<WorklogItem[]>(() =>
    focusKey ? items.filter(i => itemKey(i) === focusKey) : items.filter(isPending))
  const [index, setIndex] = useState(0)
  const [decliningWorklog, setDecliningWorklog] = useState<WorklogItem | null>(null)
  // Editing an already-decided card (focusKey on a non-pending item) starts
  // straight in edit mode — there's no swipe queue for it to sit in front of.
  const [editingKey, setEditingKey] = useState<string | null>(() =>
    queue.length && !isPending(queue[0]) ? itemKey(queue[0]) : null)
  const editOnly = queue.length > 0 && !isPending(queue[0])
  // Save/Cancel on an editOnly card close the dialog outright — falling back
  // to the swipe FABs (Approve/Dismiss) would be nonsensical for a card
  // that's already been decided.
  const finishEdit = () => editOnly ? onClose() : setEditingKey(null)

  // The queue array itself is frozen (stable order/length), but each slot's
  // DATA must stay live — otherwise an in-place edit (rematch, title edit,
  // summary edit) saves server-side but the card keeps showing the stale
  // ticket/title until the overlay is closed and reopened. `items` is already
  // patched optimistically the instant an edit is confirmed (useTimelineData's
  // `patchItem`, the single source of truth both this dialog and the timeline
  // board render from), so resolving against it here is enough — no local
  // override state needed. Falls back to the snapshot for an id that's since
  // dropped out of `items` (e.g. already reviewed elsewhere).
  const liveById = useMemo(() => new Map(items.map(i => [itemKey(i), i])), [items])
  const current = index < queue.length ? (liveById.get(itemKey(queue[index])) ?? queue[index]) : null
  const busy = current ? actions.busy === itemKey(current) : false
  const done = index >= queue.length

  const advance = () => { setIndex(i => i + 1); setDecliningWorklog(null); setEditingKey(null) }

  function commit(direction: ReviewDirection) {
    if (!current || busy) return
    if (direction === 'approve') {
      if (current.is_proposed) actions.proposedAct(current.id, 'approve')
      else actions.act(current.id, 'approve')
      advance()
      return
    }
    if (current.is_proposed) {
      actions.proposedAct(current.id, 'dismiss')
      advance()
    } else {
      setDecliningWorklog(current)
    }
  }

  function confirmDecline(correction: RejectCorrection) {
    if (!decliningWorklog) return
    actions.reject(decliningWorklog.id, correction)
    advance()
  }

  useEffect(() => {
    if (!done) return
    const id = setTimeout(onClose, 900)
    return () => clearTimeout(id)
  }, [done, onClose])

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === 'Escape') { onClose(); return }
      if (decliningWorklog || editingKey) return
      if (e.key === 'ArrowRight') commit('approve')
      if (e.key === 'ArrowLeft') commit('decline')
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [current, decliningWorklog, editingKey, onClose])

  const remaining = useMemo(() => Math.max(0, queue.length - index), [queue.length, index])

  return (
    <div className="absolute inset-0 z-50 flex items-center justify-center p-4 rise"
      style={{ background: 'rgba(20,16,40,0.5)', backdropFilter: 'blur(3px)' }} onClick={onClose}>
      <div className="w-full max-w-lg" onClick={e => e.stopPropagation()}>
        <div className="flex items-center justify-between mb-4 px-1">
          <p className="mt-label" style={{ color: '#fff' }}>
            {editOnly ? 'Edit worklog' : `Review pending ${!done ? `· ${remaining} left` : ''}`}
          </p>
          <button onClick={onClose} aria-label="Close"
            className="inline-flex items-center justify-center rounded-full"
            style={{ width: 28, height: 28, color: '#fff', background: 'rgba(255,255,255,0.16)' }}>
            <span className="text-[16px] leading-none">×</span>
          </button>
        </div>

        {/* progress dots */}
        {!done && queue.length > 1 && (
          <div className="flex items-center justify-center gap-1.5 mb-4">
            {queue.map((q, i) => (
              <span key={itemKey(q)} className="rounded-full transition-all" style={{
                width: i === index ? 20 : 6, height: 6,
                background: '#fff', opacity: i === index ? 0.95 : i < index ? 0.4 : 0.25,
              }} />
            ))}
          </div>
        )}

        {done ? (
          <div className="rounded-2xl p-10 text-center bg-card" style={{ border: '1px solid var(--t-card-border)' }}>
            <p className="mt-title" style={{ color: 'var(--color-state-approved)' }}>✓ All caught up</p>
          </div>
        ) : decliningWorklog ? (
          <ReviewRejectPicker
            worklogId={decliningWorklog.id}
            excludeKey={decliningWorklog.task_key}
            busy={busy}
            onConfirm={confirmDecline}
            onCancel={() => setDecliningWorklog(null)}
          />
        ) : (
          <AnimatePresence mode="popLayout">
            {current && (
              <ReviewCard
                key={itemKey(current)}
                item={current}
                busy={busy}
                editing={editingKey === itemKey(current)}
                onCommit={commit}
                onEditStart={() => setEditingKey(itemKey(current))}
                onEditCancel={finishEdit}
                onEditSave={async (summary, candidate) => {
                  if (current.is_proposed) {
                    await actions.saveProposedBody(current.id, summary)
                    // Still-pending (not editOnly) with a title already set —
                    // Save doubles as Approve, one action instead of a
                    // separate Save-then-Approve step. editOnly (already
                    // decided) keeps the extra checkpoint: a correction to
                    // something already reviewed shouldn't silently re-fly
                    // through without a human re-confirming.
                    if (!editOnly && current.task_title?.trim()) {
                      actions.proposedAct(current.id, 'approve')
                      advance()
                      return
                    }
                    finishEdit()
                    return
                  }
                  await actions.saveEdit(current.id, summary)
                  if (candidate && candidate.key !== current.task_key) {
                    const result = await actions.rematch(current.id, candidate)
                    if (!result.ok) return { ok: false, error: result.error }
                  }
                  if (!editOnly && summary.trim()) {
                    actions.act(current.id, 'approve')
                    advance()
                    return
                  }
                  finishEdit()
                }}
                onSaveTitle={(title) => actions.saveProposedTitle(current.id, title)}
                saveLabel={editOnly ? 'Save' : 'Save & Approve'}
              />
            )}
          </AnimatePresence>
        )}

        {!editOnly && (
          <p className="mt-body-sm mt-4 text-center" style={{ color: '#fff', opacity: 0.7 }}>
            Drag or use ←/→ · swipe right to approve, left to dismiss
          </p>
        )}
      </div>
    </div>
  )
}
