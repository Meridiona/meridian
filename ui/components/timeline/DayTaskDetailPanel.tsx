//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The right panel's task-detail state: when a day-task card is clicked in the
// timeline column, its breakdown renders HERE (in place of "Today at a glance")
// rather than as a dialog over the timeline — so the timeline keeps its clicked
// card highlighted and the rest dulled while you read the detail beside it. The
// payload is built by DayTaskColumn and threaded through MeridianTimelineShell.
//
// Layout: a flex column — the reading content (When / What was done / the draft
// preview) SCROLLS, while the "Generate worklog" / Approve action lives in a
// PINNED footer that stays visible no matter how long the summary is.
//
// This file is presentation only. The workstream palette + shared list/link
// atoms come from dayTaskKit; the worklog get/generate/approve state machine
// comes from useWorklog. Nothing here fetches or holds worklog logic directly.

'use client'

import { useEffect, useState } from 'react'
import { fmtDur } from '@/components/atoms'
import { load, mutate } from '@/lib/bridge'
import { connectedTrackers } from '@/lib/integrations'
import type { BoardTicket, DayTask, DayTasksResponse, IntegrationsResponse } from '@/lib/api-types'
import { clockLabel, type LaidSegment } from './dayTaskLayout'
import type { SettingsSection } from './settings/types'
import { Bullets, Field } from './dayTaskKit'
import { useWorklog, type WorklogState } from './useWorklog'
import { WorklogDraftDialog, WorklogEntry } from './WorklogDraftDialog'

/** Everything the right-panel detail needs about one selected workstream — built
 *  from a `LaidOutTask` by DayTaskColumn so the panel stays free of layout math. */
export interface DayTaskDetail {
  id: string
  day: string
  title: string
  minutes: number
  hue: string
  segments: LaidSegment[]
  summary: string[]
  footLo: number
  footHi: number
  linkedTicket: string | null
}

/** The selected workstream's breakdown, rendered inside the right column, with a
 *  pinned worklog action bar so Generate/Approve is always reachable. */
export function DayTaskDetailPanel({ detail, onClose, onCorrected, onOpenSettings, onOpenTask, worklog, boardTickets }: {
  detail: DayTaskDetail
  onClose: () => void
  // A dismiss/merge landed — the shell clears this selection and reloads the
  // timeline column (the corrected task is gone from it).
  onCorrected: () => void
  onOpenSettings: (section?: SettingsSection) => void
  onOpenTask: (key: string, title?: string) => void
  /** Replace the live worklog machine with a scripted one.
   *
   *  Set ONLY by the first-run walkthrough, which stands on an example day: its
   *  task ids exist in no database and its ticket keys belong to another project,
   *  so the real `generate` can only fail and the real `approve` could only file a
   *  wrong comment on somebody's board. Overriding the STATE leaves every control,
   *  label and transition below exactly as shipped - which is the point, since a
   *  tour that teaches a lookalike has taught the lookalike. */
  worklog?: WorklogState
  /** Pre-supplied tickets for the retarget picker, for the same reason - the
   *  example day's matches are not on the user's real board. */
  boardTickets?: BoardTicket[]
}) {
  const { day, id, title, minutes, hue, segments, summary, footLo, footHi } = detail
  const range = segments.length > 0 ? `${clockLabel(footLo)} - ${clockLabel(footHi)}` : ''
  // Called unconditionally (a hook cannot be conditional) and discarded when an
  // override is supplied. The cost is one `get_day_task_worklog` read that comes
  // back empty for an id the database has never seen.
  const live = useWorklog(day, id)
  const wl = worklog ?? live
  // A scripted worklog means the walkthrough is driving this panel: nothing below
  // reaches a model or a board, so the real provider/tracker pre-flights must not
  // reshape it.
  const isDemo = worklog !== undefined

  // Which PM trackers are connected — names the tracker in the CTA copy, and
  // decides whether to offer Generate or prompt the user to connect one. `null`
  // while loading so the copy doesn't flash "connect a tracker" then swap.
  const [integrations, setIntegrations] = useState<IntegrationsResponse | null>(null)
  useEffect(() => {
    let alive = true
    load<IntegrationsResponse>('/api/integrations', 'get_integrations')
      .then(r => { if (alive) setIntegrations(r) })
      .catch(() => {})
    return () => { alive = false }
  }, [])
  // Objects for the propose-branch board picker, names for the CTA copy. One read,
  // two shapes - never a second get_integrations call that could disagree.
  const connected = connectedTrackers(integrations)
  const trackers = connected.map(t => t.name)
  // Integrations loaded and nothing connected → there is nowhere to post, so the
  // entry row offers a connect instead of a dead Generate. Never in the
  // walkthrough: the tour has its own beat for the no-tracker case and drives the
  // entry row directly, so a live read of the user's integrations must not replace
  // the control a beat is waiting on.
  const noTracker = !isDemo && integrations !== null && trackers.length === 0

  // The draft opens as its own dialog rather than stacking under the evidence -
  // see `WorklogDraftDialog` for why. Closed by default on every task, including
  // one that already has a posted update: arriving on a task should show what the
  // task WAS, and a document about it is a step the user takes.
  const [draftOpen, setDraftOpen] = useState(false)
  useEffect(() => { setDraftOpen(false) }, [id])

  return (
    <div className="dt-detail h-full flex flex-col">
      {/* Scrollable reading content. */}
      <div className="flex-1 min-h-0 overflow-y-auto nice-scroll p-6 space-y-6">
        <Header title={title} minutes={minutes} hue={hue} range={range} onClose={onClose} />

        <TaskActions day={day} taskId={id} onCorrected={onCorrected} />

        {/* data-tour: inert hooks the first-run walkthrough rings, one beat each.
            These two blocks ARE the product's answer to "how would it know?" -
            the sittings it stitched together and the write-up it produced - so
            the tour names them separately rather than waving at the panel.
            See ui/components/tutorial/script.ts. */}
        {segments.length > 0 && (
          <div data-tour="detail-when" className="rounded-xl p-4 bg-card" style={{ border: '1px solid var(--t-card-border)' }}>
            <Field label="When"><SegmentList segments={segments} hue={hue} /></Field>
          </div>
        )}

        {summary.length > 0 && (
          <div data-tour="detail-done">
            <Field label="What was done"><Bullets items={summary} accent={hue} /></Field>
          </div>
        )}

      </div>

      {/* The worklog, as ONE ROW. Pinned, so it never scrolls out of reach, but a
          row rather than a document: the draft is addressed to a ticket while
          everything above is addressed to the user, and side by side in a 388px
          column the two read as one confused block. */}
      <div className="shrink-0 p-4"
        style={{ background: 'var(--t-card)', borderTop: '1px solid var(--t-card-border)', boxShadow: '0 -10px 26px -18px rgba(0,0,0,0.35)' }}>
        <WorklogEntry wl={wl} hue={hue} linkedTicket={detail.linkedTicket} noTracker={noTracker}
          onOpen={() => setDraftOpen(true)}
          onConnectTracker={() => onOpenSettings('integrations')} />
      </div>

      {draftOpen && (
        <WorklogDraftDialog wl={wl} hue={hue} taskTitle={title} linkedTicket={detail.linkedTicket}
          integrations={integrations} trackers={trackers} connected={connected}
          boardTickets={boardTickets} isDemo={isDemo}
          onClose={() => setDraftOpen(false)}
          onOpenSettings={onOpenSettings} onOpenTask={onOpenTask} />
      )}
    </div>
  )
}

/** The task header: back affordance, hue dot, title, duration + time range. */
function Header({ title, minutes, hue, range, onClose }: {
  title: string; minutes: number; hue: string; range: string; onClose: () => void
}) {
  return (
    <div>
      <button onClick={onClose}
        className="mt-body-sm inline-flex items-center gap-1.5 mb-3"
        style={{ color: 'var(--t-faint)', fontWeight: 700 }}>
        <span aria-hidden>‹</span> Back to today
      </button>
      <div className="flex items-start gap-2.5">
        <span className="mt-1.5 shrink-0 rounded-full" style={{ width: 9, height: 9, background: hue }} />
        <div className="flex-1 min-w-0">
          <p className="mt-label" style={{ color: 'var(--t-faint)' }}>Task</p>
          <p className="mt-greeting text-title mt-0.5" style={{ fontSize: 18, lineHeight: 1.3 }}>
            {title || 'Activity'}
          </p>
          <div className="flex items-center gap-2 mt-1.5">
            {minutes > 0 && (
              <span className="mt-mono-sm" style={{ fontSize: 12, fontWeight: 700, color: hue }}>{fmtDur(minutes * 60)}</span>
            )}
            {range && <span className="mt-mono-sm" style={{ fontSize: 11, color: 'var(--t-faint)' }}>{range}</span>}
          </div>
        </div>
      </div>
    </div>
  )
}

const errMsg = (e: unknown): string =>
  e instanceof Error ? e.message : typeof e === 'string' ? e : 'Something went wrong'

/** Task-level corrections to the AI's grouping: DISMISS this workstream (hide it,
 *  and keep it hidden across the hourly re-fold) or MERGE it into another one on
 *  the same day. Both persist as durable corrections server-side; here they just
 *  fire the command and let the shell reload the column via `onCorrected`.
 *
 *  Deliberately low-emphasis (ghost text buttons under the header) — these are
 *  occasional "the model got this wrong" fixes, not primary actions. Dismiss asks
 *  to confirm (it removes a card); merge opens an inline picker of the day's other
 *  tasks. On success the panel unmounts (selection clears), so busy is only reset
 *  on error. */
function TaskActions({ day, taskId, onCorrected }: {
  day: string; taskId: string; onCorrected: () => void
}) {
  const [mode, setMode] = useState<'idle' | 'confirmDismiss' | 'merge'>('idle')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [siblings, setSiblings] = useState<DayTask[] | null>(null)

  // Load the day's OTHER tasks when the merge picker opens (the reader already
  // drops dismissed ones, so this is the live set you can merge into).
  useEffect(() => {
    if (mode !== 'merge') return
    let alive = true
    setSiblings(null)
    load<DayTasksResponse>('/api/day-tasks', 'get_day_tasks', { day })
      .then(r => { if (alive) setSiblings(r.tasks.filter(t => t.id !== taskId)) })
      .catch(() => { if (alive) setSiblings([]) })
    return () => { alive = false }
  }, [mode, day, taskId])

  async function run(promise: Promise<unknown>) {
    setBusy(true); setError(null)
    try { await promise; onCorrected() }
    catch (e) { setError(errMsg(e)); setBusy(false) }
  }
  const dismiss = () =>
    run(mutate<DayTasksResponse>('/api/day-tasks', 'dismiss_day_task', { day, task_id: taskId }))
  const mergeInto = (intoId: string) =>
    run(mutate<DayTasksResponse>('/api/day-tasks', 'merge_day_task',
      { day, task_id: taskId, into_task_id: intoId }))

  return (
    <div>
      {mode === 'idle' && (
        <div className="flex items-center gap-2">
          <GhostBtn onClick={() => setMode('merge')} disabled={busy}>⧉ Merge into another task</GhostBtn>
          <GhostBtn onClick={() => setMode('confirmDismiss')} disabled={busy}>✕ Dismiss</GhostBtn>
        </div>
      )}

      {mode === 'confirmDismiss' && (
        <div className="rounded-lg px-3 py-2.5" style={{ background: 'var(--t-card)', border: '1px solid var(--t-card-border)' }}>
          <p className="mt-body-sm" style={{ color: 'var(--t-muted)', fontSize: 12.5, lineHeight: 1.5 }}>
            Hide this task from the timeline? It stays hidden even as Meridian keeps
            folding the day. You can bring it back later.
          </p>
          <div className="flex items-center gap-2 mt-2.5">
            <button onClick={dismiss} disabled={busy}
              className="mt-body-sm rounded-lg px-3 py-1.5"
              style={{ fontWeight: 700, color: '#fff', background: 'var(--color-state-pending)', opacity: busy ? 0.55 : 1, cursor: busy ? 'default' : 'pointer' }}>
              {busy ? 'Dismissing…' : 'Dismiss task'}
            </button>
            <GhostBtn onClick={() => setMode('idle')} disabled={busy}>Cancel</GhostBtn>
          </div>
        </div>
      )}

      {mode === 'merge' && (
        <div className="rounded-lg px-3 py-2.5" style={{ background: 'var(--t-card)', border: '1px solid var(--t-card-border)' }}>
          <div className="flex items-center justify-between mb-1.5">
            <p className="mt-label" style={{ color: 'var(--t-faint)' }}>Merge this task into…</p>
            <GhostBtn onClick={() => setMode('idle')} disabled={busy}>Cancel</GhostBtn>
          </div>
          {siblings === null ? (
            <p className="mt-body-sm" style={{ color: 'var(--t-faint)', fontSize: 12 }}>Loading…</p>
          ) : siblings.length === 0 ? (
            <p className="mt-body-sm" style={{ color: 'var(--t-faint)', fontSize: 12, lineHeight: 1.5 }}>
              There are no other tasks on this day to merge into.
            </p>
          ) : (
            <ul className="space-y-1 max-h-60 overflow-y-auto nice-scroll">
              {siblings.map(t => (
                <li key={t.id}>
                  <button onClick={() => mergeInto(t.id)} disabled={busy}
                    className="w-full text-left rounded-lg px-3 py-2 mt-card-hover"
                    style={{ background: 'var(--t-box)', opacity: busy ? 0.55 : 1, cursor: busy ? 'default' : 'pointer' }}>
                    <p className="mt-body-sm truncate" style={{ color: 'var(--t-title)', fontSize: 12.5, fontWeight: 600 }}>
                      {t.title || 'Activity'}
                    </p>
                    {t.minutes > 0 && (
                      <p className="mt-mono-sm mt-0.5" style={{ color: 'var(--t-faint)', fontSize: 10.5 }}>{fmtDur(t.minutes * 60)}</p>
                    )}
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}

      {error && (
        <p className="mt-body-sm mt-2" style={{ color: 'var(--color-state-pending)', fontSize: 11.5, lineHeight: 1.4 }}>{error}</p>
      )}
    </div>
  )
}

/** A low-emphasis text button — the shared shape for the task-action controls. */
function GhostBtn({ onClick, disabled, children }: {
  onClick: () => void; disabled?: boolean; children: React.ReactNode
}) {
  return (
    <button onClick={onClick} disabled={disabled}
      className="mt-body-sm inline-flex items-center gap-1 rounded-lg px-2.5 py-1"
      style={{ color: 'var(--t-muted)', border: '1px solid var(--t-hair)', fontSize: 12, opacity: disabled ? 0.55 : 1, cursor: disabled ? 'default' : 'pointer' }}>
      {children}
    </button>
  )
}

/** The "When" breakdown — one row per sitting, breaks called out between them. */
function SegmentList({ segments, hue }: { segments: LaidSegment[]; hue: string }) {
  return (
    <ul className="space-y-1">
      {segments.map((s, i) => {
        const prev = segments[i - 1]
        const gap = prev ? s.startMin - prev.endMin : 0
        return (
          <li key={i}>
            {gap > 0 && (
              <div className="flex items-center gap-2 my-1" style={{ paddingLeft: 2 }}>
                <span className="mt-mono-sm" style={{ fontSize: 9.5, color: 'var(--t-faint)', opacity: 0.8 }}>
                  break · {fmtDur(gap * 60)}
                </span>
                <span className="flex-1 border-t border-dashed" style={{ borderColor: 'var(--t-hair)' }} />
              </div>
            )}
            <div className="flex items-center gap-2.5">
              <span className="shrink-0 rounded" style={{ width: 3, height: 14, background: hue }} />
              <span className="mt-mono-sm" style={{ fontSize: 12, color: 'var(--t-muted)' }}>
                {clockLabel(s.startMin)} - {clockLabel(s.endMin)}
              </span>
              <span className="mt-mono-sm" style={{ fontSize: 10.5, color: 'var(--t-faint)' }}>
                {fmtDur((s.endMin - s.startMin) * 60)}
              </span>
            </div>
          </li>
        )
      })}
    </ul>
  )
}
