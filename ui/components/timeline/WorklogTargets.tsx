//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The tickets a worklog draft will post to, listed inside the draft preview.
//
// A draft carries 0..N targets — one strand of a day's work often advances several
// planned tasks, and the same update goes on each — so this is a LIST, and each row
// is independently removable. Lives in its own file because the detail panel is
// already near the 500-line ceiling.
//
// Presentation only: dismissing calls back up to useWorklog, which owns the write.

'use client'

import type { DayTaskWorklogDraft, WorklogTarget } from '@/lib/api-types'
import type { Tracker } from '@/lib/integrations'
import { ProviderIcon } from '@/components/ProviderIcon'
import { trackerName } from '@/lib/integrations'

/** Where the update will land: every matched ticket, or the proposed new one.
 *
 *  A ticket the USER picked (`manual`) is labelled as their choice, never with a
 *  percentage. The model's confidence is clamped to 1.0 on the way in, so a manual
 *  pick would otherwise render as "100% match" - the AI taking credit for a
 *  decision it did not make. */
export function DraftTargets({ draft, busy, trackers, onOpenTask, onDismiss, onSetProvider }: {
  draft: DayTaskWorklogDraft
  busy: boolean
  /** Connected trackers. 0 or 1 means there is no board choice to offer. */
  trackers: Tracker[]
  onOpenTask: (key: string, title?: string) => void
  onDismiss: (taskKey: string) => void
  onSetProvider: (provider: string) => void
}) {
  if (draft.propose) {
    // The provider a proposal carries is assigned as "the first configured tracker"
    // at generate time - a coin toss for anyone on two boards, and creating a ticket
    // is not undoable. So when there IS a choice, it is shown up front rather than
    // discovered afterwards in the tracker it landed on.
    return (
      <div className="rounded-lg px-3 py-2" style={{ background: 'color-mix(in srgb, var(--color-state-pending) 12%, transparent)' }}>
        <p className="mt-body-sm" style={{ color: 'var(--color-state-pending)', fontSize: 12, fontWeight: 700 }}>
          New {draft.propose.issue_type}: {draft.propose.title}
        </p>
        {draft.propose.description && (
          <p className="mt-body-sm mt-1" style={{ color: 'var(--t-muted)', fontSize: 11.5, lineHeight: 1.45 }}>{draft.propose.description}</p>
        )}
        <ProposeProvider draft={draft} trackers={trackers} busy={busy} onSetProvider={onSetProvider} />
      </div>
    )
  }
  if (draft.targets.length === 0) return <NoTarget />

  // Dismiss is only offered while the draft is still editable. Once approved, a
  // comment may already be live on the tracker and removing the row here would not
  // remove it there - it would only hide it.
  const editable = draft.state === 'drafted'
  return (
    <div className="space-y-1.5">
      {draft.targets.length > 1 && (
        <p className="mt-label" style={{ color: 'var(--t-faint)' }}>
          This update posts to all {draft.targets.length}
        </p>
      )}
      {draft.targets.map((t) => (
        <TargetRow key={t.task_key} target={t} busy={busy}
          canDismiss={editable && draft.targets.length > 0}
          onOpen={() => onOpenTask(t.task_key, t.task_title ?? undefined)}
          onDismiss={() => onDismiss(t.task_key)} />
      ))}
    </div>
  )
}

/** Whether the user may still choose the board a PROPOSED ticket is created on.
 *
 *  All three conditions are load-bearing:
 *  - a proposal (not a match): a matched draft's provider is per-target, and the
 *    core command refuses to set a draft-level one;
 *  - still `drafted`: once approved the ticket may already exist on the old board,
 *    and re-pointing the row would make it name a tracker the ticket is not on;
 *  - two or more trackers: with one there is no choice, only a label.
 *
 *  Exported for its unit tests - the repo has no React render harness, so the rule
 *  is tested here rather than through the component. */
export function canPickProposalProvider(
  draft: Pick<DayTaskWorklogDraft, 'propose' | 'state'>,
  trackerCount: number,
): boolean {
  return draft.propose !== null && draft.state === 'drafted' && trackerCount >= 2
}

/** Which board a proposed ticket gets created on.
 *
 *  Renders as a STATEMENT when there's nothing to decide - one tracker connected, or
 *  the draft already approved and the ticket already filed - and as a picker only
 *  when the choice is both real and still open. A radiogroup of chips rather than a
 *  select: two or three trackers all fit, and seeing the alternative is the point. */
function ProposeProvider({ draft, trackers, busy, onSetProvider }: {
  draft: DayTaskWorklogDraft
  trackers: Tracker[]
  busy: boolean
  onSetProvider: (provider: string) => void
}) {
  if (!canPickProposalProvider(draft, trackers.length)) {
    return (
      <p className="mt-body-sm mt-2 inline-flex items-center gap-1.5" style={{ color: 'var(--t-faint)', fontSize: 11.5 }}>
        <ProviderIcon provider={draft.provider} size={11} />
        {draft.created_task_key ? 'Created in' : 'Will be created in'} {trackerName(draft.provider)}
      </p>
    )
  }

  return (
    <div className="mt-2">
      <p className="mt-label mb-1" style={{ color: 'var(--t-faint)' }}>Create it in</p>
      <div role="radiogroup" aria-label="Which tracker to create this ticket in" className="flex flex-wrap gap-1.5">
        {trackers.map(t => {
          const picked = draft.provider === t.id
          return (
            <button key={t.id} role="radio" aria-checked={picked} disabled={busy}
              onClick={() => onSetProvider(t.id)}
              className="mt-chip inline-flex items-center gap-1.5 px-2 py-1 rounded-md transition-colors"
              style={{
                border: `1px solid ${picked ? 'var(--color-state-pending)' : 'var(--t-ctrl-border)'}`,
                background: picked ? 'color-mix(in srgb, var(--color-state-pending) 16%, transparent)' : 'var(--t-ctrl)',
                color: picked ? 'var(--t-title)' : 'var(--t-faint)',
                fontWeight: picked ? 700 : 500,
                opacity: busy ? 0.55 : 1,
                cursor: busy ? 'default' : 'pointer',
              }}>
              <ProviderIcon provider={t.id} size={12} />
              {t.name}
            </button>
          )
        })}
      </div>
    </div>
  )
}

/** One ticket the update lands on, with the reason it's here and a way out. */
function TargetRow({ target, busy, canDismiss, onOpen, onDismiss }: {
  target: WorklogTarget; busy: boolean; canDismiss: boolean
  onOpen: () => void; onDismiss: () => void
}) {
  const { task_key, task_title, confidence, manual, posted, outcome_unknown, error } = target
  const why = manual ? 'you picked this' : `${Math.round(confidence * 100)}% match`
  // Three states, and the middle one must not be dressed up as either neighbour:
  // we genuinely do not know whether this comment is live, and telling the user
  // "posted" or "not posted" would both be guesses they'd act on.
  const state = posted ? 'posted' : outcome_unknown ? 'not confirmed' : why
  return (
    <div className="flex items-stretch gap-1.5">
      <button onClick={onOpen}
        className="flex-1 min-w-0 text-left rounded-lg px-3 py-2"
        style={{ background: 'color-mix(in srgb, var(--color-state-proposal) 12%, transparent)', cursor: 'pointer' }}>
        <p className="mt-body-sm" style={{ color: 'var(--color-state-proposal)', fontSize: 12, fontWeight: 700 }}>
          {posted ? <span aria-hidden>✓ </span> : null}
          Comment on {task_key}
          <span style={{ opacity: 0.7, fontWeight: 400 }}> · {state}</span>
        </p>
        {task_title && (
          <p className="mt-body-sm mt-0.5 truncate" style={{ color: 'var(--color-state-proposal)', fontSize: 12, fontWeight: 500, opacity: 0.9 }}>
            {task_title}
          </p>
        )}
        {outcome_unknown && (
          <p className="mt-body-sm mt-0.5" style={{ color: 'var(--color-state-pending)', fontSize: 11, lineHeight: 1.4 }}>
            Meridian was interrupted while posting this and couldn&apos;t confirm it landed. Open
            the ticket to check - it won&apos;t be retried automatically, to avoid a duplicate comment.
          </p>
        )}
        {error && !outcome_unknown && (
          <p className="mt-body-sm mt-0.5" style={{ color: 'var(--color-state-pending)', fontSize: 11, lineHeight: 1.4 }}>
            {error}
          </p>
        )}
      </button>
      {canDismiss && !posted && (
        <button onClick={onDismiss} disabled={busy}
          title={`Don't post to ${task_key}`}
          aria-label={`Don't post to ${task_key}`}
          className="shrink-0 rounded-lg px-2.5"
          style={{ color: 'var(--t-faint)', border: '1px solid var(--t-hair)', fontSize: 13, opacity: busy ? 0.55 : 1, cursor: busy ? 'default' : 'pointer' }}>
          ✕
        </button>
      )}
    </div>
  )
}

/** Every match dismissed and nothing proposed - the update has nowhere to go.
 *  Said plainly, because approve will refuse and the user needs to know why. */
function NoTarget() {
  return (
    <div className="rounded-lg px-3 py-2" style={{ background: 'color-mix(in srgb, var(--color-state-pending) 10%, transparent)' }}>
      <p className="mt-body-sm" style={{ color: 'var(--color-state-pending)', fontSize: 12, fontWeight: 700 }}>
        No ticket selected
      </p>
      <p className="mt-body-sm mt-0.5" style={{ color: 'var(--t-muted)', fontSize: 11.5, lineHeight: 1.45 }}>
        Pick a ticket below to post this update, or regenerate the draft.
      </p>
    </div>
  )
}
