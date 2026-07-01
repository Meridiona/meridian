//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The swipeable review card. Physics translated from the Dayflow reference
// (SwiftUI DragGesture): rotation proportional to drag distance, a two-threshold
// model (a lower preview threshold that just shows the approve/decline stamp, a
// higher commit threshold that fires the action), and a flat fly-off exit.
// `onCommit` is called for both drag release AND the FAB buttons so there is
// exactly one code path for "an approve/decline happened." Restyled to the
// Meridian Timeline tokens; physics retuned to the mock's constants.

'use client'

import { useState } from 'react'
import { motion, useMotionValue, useTransform, type PanInfo } from 'framer-motion'
import { fmtDur, fmtClock } from '@/components/atoms'
import type { WorklogItem } from '@/lib/api-types'
import { EditableSummary } from './EditableSummary'
import { kindLabel, stateColor, stateLabel, providerLabel } from './types'

const PREVIEW_THRESHOLD = 30
const COMMIT_THRESHOLD = 120
const ROTATE_DIVISOR = 25          // mock: rotate = dx × 0.04° ⇒ divisor 1/0.04
const EXIT_DISTANCE = 760
const EXIT_ROTATE_DEG = 20
const EXIT_DURATION = 0.24         // seconds

export type ReviewDirection = 'approve' | 'decline'

export function ReviewCard({
  item, busy, editing, onCommit, onEditStart, onEditSave, onEditCancel,
}: {
  item: WorklogItem
  busy: boolean
  editing: boolean
  onCommit: (direction: ReviewDirection, velocity: number) => void
  onEditStart: () => void
  onEditSave: (summary: string) => void
  onEditCancel: () => void
}) {
  const x = useMotionValue(0)
  const rotate = useTransform(x, (v) => v / ROTATE_DIVISOR)
  const approveOpacity = useTransform(x, [PREVIEW_THRESHOLD, COMMIT_THRESHOLD], [0, 1])
  const declineOpacity = useTransform(x, [-PREVIEW_THRESHOLD, -COMMIT_THRESHOLD], [0, 1])
  const [exiting, setExiting] = useState<ReviewDirection | null>(null)

  function handleDragEnd(_: unknown, info: PanInfo) {
    if (busy || editing) return
    if (Math.abs(info.offset.x) >= COMMIT_THRESHOLD) {
      const direction: ReviewDirection = info.offset.x > 0 ? 'approve' : 'decline'
      setExiting(direction)
      onCommit(direction, info.velocity.x)
    }
  }

  const accent = stateColor(item)
  const exitX = exiting === 'approve' ? EXIT_DISTANCE : exiting === 'decline' ? -EXIT_DISTANCE : 0
  const exitRotate = exiting === 'approve' ? EXIT_ROTATE_DEG : exiting === 'decline' ? -EXIT_ROTATE_DEG : 0

  return (
    <div className="flex flex-col items-center gap-5">
      <motion.div
        drag={editing ? false : 'x'}
        dragElastic={0.65}
        dragMomentum={false}
        onDragEnd={handleDragEnd}
        animate={exiting ? { x: exitX, rotate: exitRotate, opacity: 0 } : { x: 0, rotate: 0 }}
        transition={exiting ? { duration: EXIT_DURATION, ease: 'easeIn' } : { type: 'spring', stiffness: 420, damping: 32 }}
        className="relative w-full rounded-2xl border overflow-hidden select-none bg-card"
        style={{
          x, rotate,
          borderColor: 'var(--t-card-border)',
          boxShadow: '0 20px 46px -14px rgba(20,16,40,0.34)',
          touchAction: 'pan-y',
          borderLeft: `4px solid ${accent}`,
        }}
      >
        {/* commit stamps */}
        <motion.div className="mt-chip absolute top-4 right-4 px-2.5 py-1 rounded-md pointer-events-none"
          style={{ opacity: approveOpacity, color: 'var(--color-state-approved)', border: '2px solid var(--color-state-approved)' }}>
          Approve
        </motion.div>
        <motion.div className="mt-chip absolute top-4 left-4 px-2.5 py-1 rounded-md pointer-events-none"
          style={{ opacity: declineOpacity, color: 'var(--color-state-rejected)', border: '2px solid var(--color-state-rejected)' }}>
          Dismiss
        </motion.div>

        <div className="p-5 space-y-3">
          <div className="flex items-center gap-2.5 flex-wrap">
            <span className="mt-mono-sm text-[11px] px-1.5 py-0.5 rounded bg-key-bg text-key-text">{item.task_key}</span>
            <span className="mt-body-sm" style={{ color: 'var(--t-muted)' }}>{kindLabel(item)}</span>
            <span className="mt-chip ml-auto px-2 py-0.5 rounded" style={{ color: accent, border: `1px solid ${accent}` }}>
              {stateLabel(item)}
            </span>
          </div>

          {item.task_title && <p className="mt-title-lg text-title">{item.task_title}</p>}

          {item.reasoning && (
            <div className="rounded-md p-2.5 bg-box">
              <p className="mt-label mb-1" style={{ color: 'var(--t-faint)' }}>
                {item.is_proposed ? 'Why a new ticket' : 'Why this task'}
              </p>
              <p className="mt-body-sm" style={{ color: 'var(--t-muted)' }}>{item.reasoning}</p>
            </div>
          )}

          {editing ? (
            <EditableSummary
              label="Summary"
              value={item.summary}
              placeholder="(empty — add a comment)"
              busy={busy}
              rows={3}
              onSave={onEditSave}
            />
          ) : (
            <p className="mt-body whitespace-pre-wrap" style={{ color: item.summary ? 'var(--t-title)' : 'var(--t-faint)' }}>
              {item.summary || '(empty — nothing to post)'}
            </p>
          )}

          {/* evidence footer — capture window + tracker. (WorklogItem carries no
              per-app breakdown, so the mock's app chips are represented by the
              provider chip; the time-range is the real evidence anchor.) */}
          <div className="pt-2 mt-1 border-t" style={{ borderColor: 'var(--t-hair)' }}>
            <p className="mt-label mb-1.5" style={{ color: 'var(--t-faint)' }}>Evidence from capture</p>
            <div className="flex items-center gap-2 flex-wrap">
              <span className="mt-mono-sm text-[11px]" style={{ color: 'var(--t-muted)' }}>
                {fmtClock(item.window_start)}{item.window_end ? ` – ${fmtClock(item.window_end)}` : ''}
              </span>
              <span style={{ color: 'var(--t-faint)' }}>·</span>
              <span className="mt-mono-sm text-[11px]" style={{ color: 'var(--t-muted)' }}>{fmtDur(item.time_spent_seconds)}</span>
              <span className="mt-chip ml-auto px-2 py-0.5 rounded bg-wrap" style={{ color: 'var(--t-muted)' }}>{providerLabel(item.provider)}</span>
            </div>
          </div>

          {editing && (
            <button onClick={onEditCancel} disabled={busy}
              className="mt-body-sm px-3 py-1.5 rounded-md" style={{ color: 'var(--t-muted)', border: '1px solid var(--t-hair)' }}>
              Cancel edit
            </button>
          )}
        </div>
      </motion.div>

      {/* circular FAB actions */}
      {!editing && (
        <div className="flex items-center gap-5">
          <Fab glyph="✕" label="Dismiss" size={52} color="var(--color-state-rejected)"
            onClick={() => onCommit('decline', 0)} disabled={busy} />
          <Fab glyph="✎" label="Edit" size={42} color="var(--t-muted)" faint
            onClick={onEditStart} disabled={busy} />
          <Fab glyph="✓" label="Approve" size={52} color="var(--color-state-approved)"
            onClick={() => onCommit('approve', 0)} disabled={busy} />
        </div>
      )}
    </div>
  )
}

function Fab({ glyph, label, size, color, faint, onClick, disabled }: {
  glyph: string; label: string; size: number; color: string; faint?: boolean
  onClick: () => void; disabled: boolean
}) {
  return (
    <button onClick={onClick} disabled={disabled} aria-label={label}
      className="inline-flex items-center justify-center rounded-full transition-transform active:scale-95"
      style={{
        width: size, height: size,
        color,
        background: 'var(--t-card)',
        border: `1.5px solid ${faint ? 'var(--t-hair)' : color}`,
        boxShadow: '0 6px 16px -6px rgba(20,16,40,0.3)',
        fontSize: size * 0.4,
        opacity: disabled ? 0.5 : 1,
      }}>
      {glyph}
    </button>
  )
}
