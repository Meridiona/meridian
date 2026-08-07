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

import { useState } from 'react'
import type { DayTaskWorklogDraft, GeneratedWorklogUpdate, WorklogTarget } from '@/lib/api-types'
import type { Tracker } from '@/lib/integrations'
import { ProviderIcon } from '@/components/ProviderIcon'
import { trackerName } from '@/lib/integrations'
import { CopyUpdate, UpdateBody } from './WorklogUpdateBody'

/** Where the update will land: every matched ticket, or the proposed new one.
 *
 *  A ticket the USER picked (`manual`) is labelled as their choice, never with a
 *  percentage. The model's confidence is clamped to 1.0 on the way in, so a manual
 *  pick would otherwise render as "100% match" - the AI taking credit for a
 *  decision it did not make. */
export function DraftTargets({ draft, busy, trackers, flat = false, onOpenTask, onDismiss, onSetProvider }: {
  draft: DayTaskWorklogDraft
  busy: boolean
  /** Connected trackers. 0 or 1 means there is no board choice to offer. */
  trackers: Tracker[]
  /** Rendered INSIDE `DraftDocument`'s card, which is already the surface: drop
   *  the tinted panel and set the text as body copy. Standalone (the default) it
   *  still needs its own edges. */
  flat?: boolean
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
      <div className={flat ? undefined : 'rounded-lg px-3 py-2'}
        style={flat ? undefined : { background: 'color-mix(in srgb, var(--color-state-pending) 12%, transparent)' }}>
        {/* THE TITLE IS THE THING BEING DECIDED, so it is the largest text in
            the block and the issue type is a chip beside it rather than a
            prefix inside it. It used to run "New Task: <title>" as one string,
            which buried the title mid-sentence behind boilerplate. */}
        <div className="flex items-baseline gap-2 flex-wrap">
          {flat && (
            <span className="mt-label shrink-0 rounded px-1.5 py-0.5" style={{
              fontSize: 9.5, color: 'var(--color-state-pending)',
              background: 'color-mix(in srgb, var(--color-state-pending) 14%, transparent)',
            }}>
              {draft.propose.issue_type}
            </span>
          )}
          <p className="mt-body-sm min-w-0" style={{
            color: flat ? 'var(--t-title)' : 'var(--color-state-pending)',
            fontSize: flat ? 15 : 12, fontWeight: 700,
            letterSpacing: flat ? '-0.01em' : undefined, lineHeight: 1.3,
          }}>
            {flat ? draft.propose.title : `New ${draft.propose.issue_type}: ${draft.propose.title}`}
          </p>
        </div>
        {draft.propose.description && (
          flat
            ? <TicketDescription text={draft.propose.description} />
            : <p className="mt-body-sm mt-1.5" style={{
              color: 'var(--t-muted)', fontSize: 11.5, lineHeight: 1.45, fontWeight: 400,
            }}>{draft.propose.description}</p>
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
  // Per-ticket bodies: when the model split the work (any target carries its own
  // update), each row shows ITS body - they are different, and posting one shared
  // block below would contradict that. When none does (a single match, a legacy
  // draft, the fallback), the shared body stays below the list and rows stay compact.
  const perTicket = draft.targets.some((t) => t.update != null)
  return (
    // data-tour: the first-run walkthrough rings this block when it explains what
    // "matched" means and how far it is allowed to have looked. Inert otherwise.
    <div data-tour="draft-targets" className="space-y-1.5">
      {draft.targets.length > 1 && (
        <p className="mt-label" style={{ color: 'var(--t-faint)' }}>
          {perTicket
            ? `${draft.targets.length} tickets, each with its own update`
            : `This update posts to all ${draft.targets.length}`}
        </p>
      )}
      {draft.targets.map((t) => (
        <TargetRow key={t.task_key} target={t} busy={busy} flat={flat}
          canDismiss={editable && draft.targets.length > 0}
          body={perTicket ? (t.update ?? draft.update) : null}
          onOpen={() => onOpenTask(t.task_key, t.task_title ?? undefined)}
          onDismiss={() => onDismiss(t.task_key)} />
      ))}
    </div>
  )
}

/** The whole draft, laid out as the document it is.
 *
 *  NO CARD. Successive versions of this were a card, then two cards, then one
 *  card with internal header rows - and every one of them was a framed surface
 *  drawn inside a dialog that is already a framed surface with its own title bar
 *  and its own footer. Two chromes nested, the inner one repeating the outer
 *  one's job: a header saying "THE UPDATE" directly beneath a title bar saying
 *  "Worklog draft", a border inside a border, a tinted panel on a tinted panel.
 *  The dialog IS the document. This is its contents, and nothing else.
 *
 *  THE UPDATE LEADS, the destination follows. The proposal used to open the
 *  dialog, so the reader met a model-written ticket body before the thing they
 *  are being asked to judge - and "where does it go" is a question you ask AFTER
 *  you know what "it" says. Putting the destination last also puts it directly
 *  above the button that acts on it, which is where proximity should have had it
 *  all along.
 *
 *  # Who calls this
 *  [`WorklogDraftDialog`]'s body, and the walkthrough's `TutorialSummaryTask`,
 *  which is the same surface by design. */
export function DraftDocument({ draft, busy, trackers, onOpenTask, onDismiss, onSetProvider }: {
  draft: DayTaskWorklogDraft
  busy: boolean
  trackers: Tracker[]
  onOpenTask: (key: string, title?: string) => void
  onDismiss: (taskKey: string) => void
  onSetProvider: (provider: string) => void
}) {
  return (
    <div>
      {/* The document itself, starting immediately - no label, because the
          dialog's own title already says what this is. */}
      {!hasPerTicketUpdates(draft) && (
        <div data-tour="sum-update">
          <UpdateBody update={draft.update} boxed unframed />
        </div>
      )}

      {/* Where it lands. A single tinted band rather than a card: it is a
          different KIND of thing from the prose above it (a destination, not a
          document), and one change of surface says that more quietly than a
          border, a header row and a heading all saying it at once. */}
      <div data-tour="sum-where" className="mt-6 rounded-xl px-4 py-3.5" style={{
        background: 'var(--t-box)',
      }}>
        <p className="mt-label mb-2.5" style={{ color: 'var(--t-faint-2)' }}>
          {draft.propose ? 'CREATES A NEW TICKET' : 'GOES TO'}
        </p>
        <DraftTargets draft={draft} busy={busy} trackers={trackers} flat
          onOpenTask={onOpenTask} onDismiss={onDismiss} onSetProvider={onSetProvider} />
      </div>
    </div>
  )
}

/** How much proposed-ticket description shows before it is folded away. Roughly
 *  three lines at the card's width - enough to tell what the ticket is about. */
const DESCRIPTION_PEEK = 190

/** The proposed ticket's description, folded when it is long.
 *
 *  IT IS REFERENCE, NOT THE DECISION. The model writes a full ticket body -
 *  scope, first target, what else is in play - and at a hundred-odd words it was
 *  the tallest block in the dialog, sitting ABOVE the update and pushing the
 *  thing the user is actually judging below the fold. Worse, it largely restates
 *  that update in a different register, so the reader meets the same work twice
 *  and has to work out which one they are being asked about.
 *
 *  So: a peek, and a way to read the rest. Nothing is hidden that changes the
 *  decision - the title says what the ticket is, and this says how it will be
 *  written up once it exists. */
function TicketDescription({ text }: { text: string }) {
  const long = text.length > DESCRIPTION_PEEK
  const [open, setOpen] = useState(false)
  return (
    <div className="mt-2">
      <p className="mt-body-sm" style={{
        color: 'var(--t-muted)', fontSize: 12.5, lineHeight: 1.55, fontWeight: 400,
        ...(long && !open
          ? { display: '-webkit-box', WebkitLineClamp: 3, WebkitBoxOrient: 'vertical' as const, overflow: 'hidden' }
          : null),
      }}>
        {text}
      </p>
      {long && (
        <button onClick={() => setOpen(!open)}
          className="mt-1.5"
          style={{
            fontSize: 11.5, fontWeight: 700, color: 'var(--color-state-proposal)',
            background: 'transparent', border: 'none', cursor: 'pointer', padding: 0,
          }}>
          {open ? 'Show less' : 'Read the full description'}
        </button>
      )}
    </div>
  )
}

/** Whether the draft carries a distinct per-ticket update on any target. Exported
 *  so the panel can decide whether to ALSO show the shared body block (it must not
 *  when the bodies live in the rows). */
export function hasPerTicketUpdates(draft: DayTaskWorklogDraft): boolean {
  return draft.targets.some((t) => t.update != null)
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

/** One ticket the update lands on, with the reason it's here and a way out.
 *  `body`, when set, is THIS ticket's own update rendered inline (the multi-match
 *  split) - null when the shared body is shown once below the whole list instead. */
function TargetRow({ target, busy, canDismiss, body, flat = false, onOpen, onDismiss }: {
  target: WorklogTarget; busy: boolean; canDismiss: boolean
  body: GeneratedWorklogUpdate | null
  /** Inside `DraftDocument`'s single card - no tint of its own. */
  flat?: boolean
  onOpen: () => void; onDismiss: () => void
}) {
  const { task_key, task_title, provider, confidence, manual, posted, outcome_unknown, error } = target
  const isPersonal = provider === 'local'
  const why = manual ? 'you picked this' : `${Math.round(confidence * 100)}% match`
  // Three states, and the middle one must not be dressed up as either neighbour:
  // we genuinely do not know whether this comment is live, and telling the user
  // "posted" or "not posted" would both be guesses they'd act on.
  const state = posted ? (isPersonal ? 'logged' : 'posted') : outcome_unknown ? 'not confirmed' : why
  // THE TICKET'S OWN TITLE LEADS. This row used to open "Comment on MER-475 ·
  // 90% match" in bold and drop the actual ticket name underneath it, smaller and
  // dimmer - so the largest text on the row described the MECHANISM (that a
  // comment is the delivery method) while the one thing the user has to judge,
  // "is this the right ticket?", was the quietest thing in the box. The key, the
  // tracker's mark and the confidence are all still here; they are metadata about
  // the title, and they now read that way.
  const tone = posted ? 'var(--color-state-approved)'
    : outcome_unknown ? 'var(--color-state-pending)'
      : 'var(--color-state-proposal)'
  return (
    <div className="flex items-stretch gap-1.5">
      <button onClick={onOpen}
        className={`flex-1 min-w-0 text-left rounded-lg ${flat ? '' : 'px-3 py-2.5'}`}
        style={{ background: flat ? 'transparent' : `color-mix(in srgb, ${tone} 10%, transparent)`, cursor: 'pointer' }}>
        <p className="mt-body-sm" style={{ color: 'var(--t-title)', fontSize: 13.5, fontWeight: 700, lineHeight: 1.35 }}>
          {task_title || task_key}
        </p>
        {/* One quiet meta line: whose board, which ticket, how sure. The tracker
            is its own mark rather than a word - it is recognised faster than it is
            read, and it keeps the line short enough to take in at a glance. */}
        <span className="flex items-center gap-1.5 mt-1.5">
          {!isPersonal && <ProviderIcon provider={provider} size={13} />}
          <span style={{ color: 'var(--t-muted)', fontSize: 11.5, fontWeight: 600, fontFamily: 'var(--font-jetbrains-mono), monospace' }}>
            {isPersonal ? 'Personal task' : task_key}
          </span>
          <span aria-hidden style={{ color: 'var(--t-faint)', fontSize: 11 }}>·</span>
          <span style={{ color: tone, fontSize: 11.5, fontWeight: 700 }}>
            {posted ? <span aria-hidden>✓ </span> : null}{state}
          </span>
        </span>
        {isPersonal && (
          <p className="mt-body-sm mt-0.5" style={{ color: 'var(--t-faint)', fontSize: 11, lineHeight: 1.4 }}>
            Personal task - not logged to your PM provider. Open it to create a ticket and post, or match it to an existing one.
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
        {body && <UpdateBody update={body} />}
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
