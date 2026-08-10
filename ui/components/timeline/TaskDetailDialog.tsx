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
// A personal (provider 'local') task additionally gets an inline Edit affordance
// for its title/description (see `editableTask`) — tracker-owned tickets never do.
// The status badge is always a live `StatusPicker` (same component + hook as the
// Tasks page's TasksDetailPane) — moving a tracker ticket writes through to the
// tracker, and moving a personal task's To Do/In Progress/Done writes straight to
// our own DB; both get the same optimistic patch + Undo banner.

'use client'

import { useEffect, useState } from 'react'
import { LOCAL_PROVIDER, type TaskDetail, type TaskStatusTarget, type EscalateResponse } from '@/lib/api-types'
import { load, invoke, openExternal } from '@/lib/bridge'
import { editTask } from '@/components/plan/useTaskComposer'
import { refreshPlan } from '@/components/plan/planStore'
import { StatusPicker } from './StatusPicker'
import { useTaskStatusChange, StatusBanner } from './useTaskStatusChange'
import { WorklogTicketPicker } from './WorklogTicketPicker'

export function TaskDetailDialog({
  taskKey, fallbackTitle, onClose, inToday, canEdit = true, onAdd, onRemove, onDeleted, day, editableTask = true,
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
  // Fired after a personal task is deleted (see `canEditText`/delete below), right
  // before the dialog closes itself — lets the caller drop it from whatever list
  // it's currently showing without waiting on that list's own next refetch.
  onDeleted?: () => void
  // The day this dialog was opened from — used to refresh that day's plan
  // store after a title/description edit lands, so a "Today's focus" card
  // showing this task updates without a manual poll.
  day?: string
  // Whether a personal (provider 'local') task's title/description can be
  // rewritten from here. Defaults true; callers set it false when the task
  // was opened from a PAST day's Today's-focus checklist — that day's plan
  // is a record of what was focused on, not something to rewrite after the
  // fact. Has no effect on tracker-owned tickets, which are never editable
  // here regardless.
  editableTask?: boolean
}) {
  const [detail, setDetail] = useState<TaskDetail | null>(null)
  const [loading, setLoading] = useState(true)
  const [editing, setEditing] = useState(false)
  const [draftTitle, setDraftTitle] = useState('')
  const [draftDescription, setDraftDescription] = useState('')
  const [saving, setSaving] = useState(false)
  const [saveError, setSaveError] = useState<string | null>(null)
  // Deleting a personal task: a confirm step (it disappears from every list, not
  // just today's), then the write.
  const [confirmingDelete, setConfirmingDelete] = useState(false)
  const [deleting, setDeleting] = useState(false)
  const [deleteError, setDeleteError] = useState<string | null>(null)
  // Escalation: promote this personal task onto a real tracker (create a new
  // ticket, or match an existing one) and post its auto-logged update there.
  const [escalating, setEscalating] = useState(false)
  const [escalateError, setEscalateError] = useState<string | null>(null)
  const [picking, setPicking] = useState(false)

  // Same status-change plumbing as the Tasks page's detail pane
  // (TasksDetailPane.tsx) — the mutate call, optimistic patch, and Undo/redirect
  // banner all live in the shared hook, so this dialog gets identical behavior
  // (including a personal task's To Do/In Progress/Done lifecycle, which
  // `set_task_status` now also serves — see meridian_core::task_create's
  // local-status helpers) rather than a second implementation.
  const patchTaskStatus = (key: string, status: string, isTerminal: boolean) =>
    setDetail(d => d && d.key === key ? { ...d, status, is_terminal: isTerminal } : d)
  const { statusBusyKey, undo, note, handleSetStatus, handleUndo, dismissUndo, dismissNote } =
    useTaskStatusChange(patchTaskStatus)
  const statusTarget: TaskStatusTarget | null = detail
    ? { key: detail.key, provider: detail.provider, status: detail.status, is_terminal: detail.is_terminal }
    : null

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
  // Only a personal task's own text is ours to rewrite — a tracker-owned
  // ticket's title/description stays read-only here (edit it in the tracker).
  const isPersonal = detail?.provider === LOCAL_PROVIDER
  const canEditText = isPersonal && editableTask
  // Deletion is available for every personal task, even one on a past-day plan
  // (editableTask=false): removing a mistakenly created task must not be gated
  // on the historical text-edit permission. Editing stays gated by canEditText.
  const canDeleteTask = isPersonal

  function startEdit() {
    setDraftTitle(detail?.title ?? '')
    setDraftDescription(detail?.description ?? '')
    setSaveError(null)
    setEditing(true)
  }

  function saveEdit() {
    if (!draftTitle.trim() || saving) return
    setSaving(true)
    setSaveError(null)
    editTask(day ?? '', taskKey, { title: draftTitle.trim(), description: draftDescription })
      .then(() => {
        setDetail(d => d && { ...d, title: draftTitle.trim(), description: draftDescription })
        setEditing(false)
      })
      .catch(e => setSaveError(e instanceof Error ? e.message : 'Couldn’t save the task'))
      .finally(() => setSaving(false))
  }

  /** Delete this personal task: it stops appearing anywhere it's listed (today's
   *  plan, the Tasks board, suggestions) — see `meridian_core::task_create`'s
   *  `delete_local_task` for why this is a hide, not a hard DB delete.
   *
   *  This dialog is a shared modal layered on top of whatever surface opened it
   *  (the Tasks board, the daily plan, a timeline card) — it never remounts that
   *  surface, so its own already-fetched list would otherwise stay stale after a
   *  successful delete. `onDeleted` covers the one caller that wires it
   *  (PlanView); the `meridian:task-deleted` window event (same pattern as
   *  NoticeBar's `meridian:open-settings`) is the broadcast every OTHER list-owning
   *  surface listens for instead of threading a callback through every opener. */
  function deleteTask() {
    if (deleting) return
    setDeleting(true)
    setDeleteError(null)
    invoke('delete_plan_task', { taskKey })
      .then(() => {
        window.dispatchEvent(new CustomEvent('meridian:task-deleted', { detail: { taskKey } }))
        onDeleted?.()
        onClose()
      })
      .catch(e => {
        setDeleteError(e instanceof Error ? e.message : 'Couldn’t delete the task')
        setDeleting(false)
      })
  }

  /** The task just GRADUATED into a real ticket - it isn't personal anymore.
   *  Morph the open dialog into that ticket in place (new key/provider/url) so it
   *  stops offering escalation and shows itself as the real ticket; the logged
   *  update rides along on the row, so it stays visible. Later opens come from the
   *  now-repointed plan already keyed to the real ticket. */
  function onEscalated(res: EscalateResponse) {
    setDetail(d => d && { ...d, key: res.linked_ticket, provider: res.provider, url: res.browse_url ?? '' })
    setPicking(false)
    // The plan is now keyed to the real ticket - refresh Today's focus so the card
    // stops showing this as a personal task.
    if (day) refreshPlan(day)
  }

  function convertToTicket() {
    if (escalating) return
    setEscalating(true)
    setEscalateError(null)
    invoke<EscalateResponse>('escalate_personal_task_create', { taskKey })
      .then(onEscalated)
      .catch(e => setEscalateError(e instanceof Error ? e.message : 'Couldn’t create the ticket'))
      .finally(() => setEscalating(false))
  }

  function matchToTicket(targetKey: string) {
    if (escalating) return
    setEscalating(true)
    setEscalateError(null)
    invoke<EscalateResponse>('escalate_personal_task_match', { taskKey, targetKey })
      .then(onEscalated)
      .catch(e => setEscalateError(e instanceof Error ? e.message : 'Couldn’t post to that ticket'))
      .finally(() => setEscalating(false))
  }

  return (
    <div className="absolute inset-0 z-50 flex items-start justify-center p-6 sm:p-10 rise"
      style={{ background: 'rgba(20,16,40,0.5)', backdropFilter: 'blur(3px)' }} onClick={onClose}>
      <div className="w-full rounded-2xl overflow-hidden flex flex-col bg-panel"
        style={{ maxWidth: 640, maxHeight: '88%', border: '1px solid var(--t-card-border)', boxShadow: 'var(--mt-modal-shadow)' }}
        onClick={e => e.stopPropagation()}>
        <div className="px-7 pt-6 pb-5 border-b shrink-0" style={{ borderColor: 'var(--t-hair)' }}>
          <div className="flex items-center gap-2 mb-2.5">
            <span className="mt-mono-sm text-[11px] px-1.5 py-0.5 rounded bg-key-bg text-key-text">{detail?.key ?? taskKey}</span>
            {detail?.issue_type && (
              <span className="mt-chip px-1.5 py-0.5 rounded" style={{ color: 'var(--t-muted)', border: '1px solid var(--t-hair)' }}>
                {detail.issue_type}
              </span>
            )}
            {statusTarget && (
              <StatusPicker task={statusTarget} onPick={o => handleSetStatus(statusTarget, o)}
                busy={statusBusyKey === statusTarget.key} />
            )}
            <button onClick={onClose} aria-label="Close"
              className="ml-auto inline-flex items-center justify-center rounded-full bg-wrap shrink-0"
              style={{ width: 28, height: 28, color: 'var(--t-muted)' }}>
              <span className="text-[16px] leading-none">×</span>
            </button>
          </div>
          {editing ? (
            <input autoFocus value={draftTitle} onChange={e => setDraftTitle(e.target.value)}
              placeholder="Task title" className="mt-modal-title text-title w-full bg-transparent outline-none"
              style={{ border: '1px solid var(--t-ctrl-border)', borderRadius: 8, padding: '4px 8px' }} />
          ) : (
            <p className="mt-modal-title text-title">{title}</p>
          )}
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

        <div className="overflow-y-auto nice-scroll p-7 space-y-5">
          {loading && !detail ? (
            <p className="mt-body-sm italic" style={{ color: 'var(--t-faint-2)' }}>Loading…</p>
          ) : (
            <>
              <div>
                <p className="mt-label mb-2" style={{ color: 'var(--t-faint)' }}>Description</p>
                {editing ? (
                  <textarea autoFocus={false} value={draftDescription} onChange={e => setDraftDescription(e.target.value)}
                    placeholder="Add a description…" rows={5}
                    className="mt-body w-full bg-transparent outline-none resize-none"
                    style={{ color: 'var(--t-muted)', border: '1px solid var(--t-ctrl-border)', borderRadius: 8, padding: '8px 10px' }} />
                ) : detail?.description?.trim() ? (
                  <p className="mt-body whitespace-pre-wrap" style={{ color: 'var(--t-muted)' }}>{detail.description}</p>
                ) : (
                  <p className="mt-body-sm italic" style={{ color: 'var(--t-faint-2)' }}>No description on this ticket.</p>
                )}
                {editing && saveError && (
                  <p className="mt-body-sm mt-2" style={{ color: 'var(--color-state-pending)' }}>{saveError}</p>
                )}
              </div>
              {detail?.acceptance_criteria && (
                <div>
                  <p className="mt-label mb-2" style={{ color: 'var(--t-faint)' }}>Acceptance criteria</p>
                  <p className="mt-body whitespace-pre-wrap" style={{ color: 'var(--t-muted)' }}>{detail.acceptance_criteria}</p>
                </div>
              )}
              {detail?.local_worklog_text && (
                <div className="rounded-xl p-4" style={{ background: 'color-mix(in srgb, var(--accent) 7%, transparent)', border: '1px solid var(--t-hair)' }}>
                  <div className="flex items-center gap-2 mb-2">
                    <p className="mt-label" style={{ color: 'var(--t-faint)' }}>Logged update</p>
                    <span className="mt-chip px-1.5 py-0.5 rounded" style={{ background: 'color-mix(in srgb, var(--accent) 14%, transparent)', color: 'var(--accent)' }}>
                      auto-logged
                    </span>
                    {detail.local_worklog_posted_at && (
                      <span className="mt-body-sm ml-auto" style={{ color: 'var(--t-faint-2)' }}>{fmtWhen(detail.local_worklog_posted_at)}</span>
                    )}
                  </div>
                  <p className="mt-body whitespace-pre-wrap" style={{ color: 'var(--t-muted)' }}>{detail.local_worklog_text}</p>

                  {/* Escalation. A personal task's update lives only on the task -
                      promote it onto a real tracker if the work belongs there, which
                      GRADUATES it into that ticket (it stops being personal). Once
                      graduated the dialog is showing the real ticket, so the buttons
                      give way to a note that the work is now tracked there. */}
                  <div className="mt-3 pt-3" style={{ borderTop: '1px solid var(--t-hair)' }}>
                    {!isPersonal ? (
                      <p className="mt-body-sm inline-flex items-center gap-1.5" style={{ color: 'var(--t-muted)' }}>
                        <span aria-hidden>↗</span> Now tracked on your board as
                        {detail.url ? (
                          <button onClick={() => openExternal(detail.url)}
                            title={`Open ${detail.key}`}
                            className="mt-mono-sm px-1.5 py-0.5 rounded bg-key-bg text-key-text"
                            style={{ cursor: 'pointer', textDecoration: 'underline', textUnderlineOffset: 2 }}>
                            {detail.key}
                          </button>
                        ) : (
                          <span className="mt-mono-sm px-1.5 py-0.5 rounded bg-key-bg text-key-text">{detail.key}</span>
                        )}
                      </p>
                    ) : picking ? (
                      <WorklogTicketPicker
                        current={null}
                        busy={escalating}
                        excludeLocal
                        title="Match this to which ticket?"
                        onPick={matchToTicket}
                        onCancel={() => setPicking(false)}
                      />
                    ) : (
                      <>
                        <p className="mt-body-sm mb-2" style={{ color: 'var(--t-faint)' }}>
                          This is a personal task, so it isn&apos;t logged to your PM provider. Create a ticket and post it, or match it to an existing one - either way it becomes a real ticket on your board.
                        </p>
                        <div className="flex items-center gap-2">
                          <button onClick={convertToTicket} disabled={escalating}
                            className="mt-body-sm px-3 py-1.5 rounded-lg"
                            style={{ background: 'var(--t-title)', color: 'var(--t-panel)', fontWeight: 700, opacity: escalating ? 0.6 : 1 }}>
                            {escalating ? 'Working…' : 'Create a ticket & post'}
                          </button>
                          <button onClick={() => { setEscalateError(null); setPicking(true) }} disabled={escalating}
                            className="mt-body-sm px-3 py-1.5 rounded-lg bg-ctrl"
                            style={{ border: '1px solid var(--t-ctrl-border)', color: 'var(--t-muted)', opacity: escalating ? 0.6 : 1 }}>
                            Match to existing ticket
                          </button>
                        </div>
                      </>
                    )}
                    {escalateError && (
                      <p className="mt-body-sm mt-2" style={{ color: 'var(--color-state-pending)' }}>{escalateError}</p>
                    )}
                  </div>
                </div>
              )}
            </>
          )}
        </div>

        {confirmingDelete ? (
          <div className="px-7 py-5 border-t shrink-0" style={{ borderColor: 'var(--t-hair)' }}>
            <p className="mt-body-sm mb-3" style={{ color: 'var(--t-muted)' }}>
              Delete this task? It disappears from today&apos;s plan, the Tasks board,
              and future suggestions. This can&apos;t be undone from here.
            </p>
            {deleteError && (
              <p className="mt-body-sm mb-3" style={{ color: 'var(--color-state-pending)' }}>{deleteError}</p>
            )}
            <div className="flex items-center gap-2.5">
              <button onClick={deleteTask} disabled={deleting}
                className="mt-body-sm px-3.5 py-2 rounded-lg"
                style={{ background: 'var(--color-state-pending)', color: '#fff', opacity: deleting ? 0.6 : 1 }}>
                {deleting ? 'Deleting…' : 'Yes, delete'}
              </button>
              <button onClick={() => setConfirmingDelete(false)} disabled={deleting}
                className="mt-body-sm px-3.5 py-2 rounded-lg bg-ctrl"
                style={{ border: '1px solid var(--t-ctrl-border)', color: 'var(--t-muted)' }}>
                Cancel
              </button>
            </div>
          </div>
        ) : (detail?.url || onAdd || onRemove || canEditText || canDeleteTask) && (
          <div className="px-7 py-5 border-t shrink-0 flex items-center gap-2.5" style={{ borderColor: 'var(--t-hair)' }}>
            {canEditText && (editing ? (
              <>
                <button onClick={saveEdit} disabled={saving || !draftTitle.trim()}
                  className="mt-body-sm px-3.5 py-2 rounded-lg"
                  style={{ background: 'var(--color-state-approved)', color: '#fff', opacity: saving || !draftTitle.trim() ? 0.6 : 1 }}>
                  {saving ? 'Saving…' : 'Save'}
                </button>
                <button onClick={() => setEditing(false)} disabled={saving}
                  className="mt-body-sm px-3.5 py-2 rounded-lg bg-ctrl"
                  style={{ border: '1px solid var(--t-ctrl-border)', color: 'var(--t-muted)' }}>
                  Cancel
                </button>
              </>
            ) : (
              <button onClick={startEdit}
                className="mt-body-sm px-3.5 py-2 rounded-lg bg-ctrl"
                style={{ border: '1px solid var(--t-ctrl-border)', color: 'var(--t-muted)' }}>
                Edit
              </button>
            ))}
            {canDeleteTask && !editing && (
              <button onClick={() => setConfirmingDelete(true)}
                className="mt-body-sm px-3.5 py-2 rounded-lg bg-ctrl"
                style={{ border: '1px solid var(--t-ctrl-border)', color: 'var(--color-state-pending)' }}>
                Delete
              </button>
            )}
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
              <a href={detail.url} onClick={(e) => { e.preventDefault(); openExternal(detail!.url!) }}
                className="mt-body-sm px-3.5 py-2 rounded-lg inline-block bg-ctrl ml-auto"
                style={{ border: '1px solid var(--t-ctrl-border)', color: 'var(--t-muted)' }}>
                Open in tracker ↗
              </a>
            )}
          </div>
        )}
      </div>
      <div onClick={e => e.stopPropagation()}>
        <StatusBanner undo={undo} note={note} busy={!!statusBusyKey}
          onUndo={handleUndo} onDismissUndo={dismissUndo} onDismissNote={dismissNote} />
      </div>
    </div>
  )
}

/** "logged 2:14 PM" style stamp for the auto-logged update - date only when it
 *  wasn't today, so a same-day log reads as a time and an older one as a date. */
function fmtWhen(iso: string): string {
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return ''
  const now = new Date()
  const sameDay = d.toDateString() === now.toDateString()
  return sameDay
    ? d.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })
    : d.toLocaleDateString([], { month: 'short', day: 'numeric' })
}

function MetaBit({ label, value, warn = false }: { label: string; value: string; warn?: boolean }) {
  return (
    <span className="mt-body-sm" style={{ color: warn ? 'var(--color-state-pending)' : 'var(--t-muted)' }}>
      <span style={{ color: 'var(--t-faint)' }}>{label}: </span>{value}
    </span>
  )
}
