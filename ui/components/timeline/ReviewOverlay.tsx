//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The "Review pending" swipe dialog — a queue of drafted worklogs + proposed
// tickets presented one at a time. Owns its own queue snapshot + index so an
// in-flight review isn't reshuffled by useTimelineData's 30s poll; every action
// flows through the shared mutation callbacks, so the timeline updates
// automatically. Self-contained backdrop (ReviewModal just mounts this).

'use client'

import { useEffect, useMemo, useState } from 'react'
import { AnimatePresence } from 'framer-motion'
import type { WorklogItem } from '@/lib/api-types'
import { ReviewCard, type ReviewDirection } from './ReviewCard'
import { ReviewRejectPicker } from './ReviewRejectPicker'
import { itemKey, isPending, type RejectCorrection } from './types'
import type { WorklogActions } from './useTimelineData'

export function ReviewOverlay({ items, actions, onClose }: {
  items: WorklogItem[]
  actions: WorklogActions
  onClose: () => void
}) {
  // Snapshot at open time — see file header for why.
  const [queue] = useState<WorklogItem[]>(() => items.filter(isPending))
  const [index, setIndex] = useState(0)
  const [decliningWorklog, setDecliningWorklog] = useState<WorklogItem | null>(null)
  const [editingKey, setEditingKey] = useState<string | null>(null)

  const current = index < queue.length ? queue[index] : null
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
            Review pending {!done && `· ${remaining} left`}
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
                onEditCancel={() => setEditingKey(null)}
                onEditSave={(summary) => {
                  if (current.is_proposed) actions.saveProposedBody(current.id, summary)
                  else actions.saveEdit(current.id, summary)
                  setEditingKey(null)
                }}
              />
            )}
          </AnimatePresence>
        )}

        <p className="mt-body-sm mt-4 text-center" style={{ color: '#fff', opacity: 0.7 }}>
          Drag or use ←/→ · swipe right to approve, left to dismiss
        </p>
      </div>
    </div>
  )
}
