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
import { Bullets, Field } from './dayTaskKit'

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
        <p className="mt-body-sm" style={{
          color: flat ? 'var(--t-title)' : 'var(--color-state-pending)',
          fontSize: flat ? 13.5 : 12, fontWeight: 700, lineHeight: 1.35,
        }}>
          New {draft.propose.issue_type}: {draft.propose.title}
        </p>
        {draft.propose.description && (
          <p className="mt-body-sm mt-1.5" style={{
            color: flat ? 'var(--t-title)' : 'var(--t-muted)',
            fontSize: flat ? 13 : 11.5, lineHeight: flat ? 1.6 : 1.45, fontWeight: 400,
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

/** The whole draft as ONE document: where it lands, then what lands there.
 *
 *  These used to be two cards stacked with a gap - an amber-tinted proposal panel
 *  above a lilac update box - which said, in the only language a layout has, that
 *  they were two separate things to consider. They are not: the ticket and the
 *  text that goes on it are one object, and the user's decision is about the pair.
 *  Two frames in two colours also meant two competing surfaces in a 640px dialog,
 *  and the eye had nowhere to start.
 *
 *  One card, one background, one border, with hairlines between its parts. The
 *  parts keep their own labels, because "where" and "what" are still different
 *  questions - they are just being asked on the same page.
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
  const hair = { borderColor: 'var(--t-hair)' }
  return (
    <section className="rounded-xl overflow-hidden" style={{
      background: 'var(--t-box)', border: '1px solid var(--t-card-border)',
    }}>
      <div data-tour="sum-where" className="px-4 pt-3.5 pb-4 border-b" style={hair}>
        <p className="mt-label mb-2" style={{ color: 'var(--t-muted)' }}>
          {draft.propose ? 'NEW TICKET - NOTHING ON YOUR PLAN MATCHED' : 'WHERE THIS GOES'}
        </p>
        <DraftTargets draft={draft} busy={busy} trackers={trackers} flat
          onOpenTask={onOpenTask} onDismiss={onDismiss} onSetProvider={onSetProvider} />
      </div>
      {!hasPerTicketUpdates(draft) && (
        <div data-tour="sum-update">
          <header className="flex items-center justify-between gap-3 px-4 py-3 border-b" style={hair}>
            <p className="mt-label" style={{ color: 'var(--t-muted)' }}>THE UPDATE</p>
            <CopyUpdate update={draft.update} />
          </header>
          <div className="px-4 py-3.5"><UpdateBody update={draft.update} boxed unframed /></div>
        </div>
      )}
    </section>
  )
}

/** Whether the draft carries a distinct per-ticket update on any target. Exported
 *  so the panel can decide whether to ALSO show the shared body block (it must not
 *  when the bodies live in the rows). */
export function hasPerTicketUpdates(draft: DayTaskWorklogDraft): boolean {
  return draft.targets.some((t) => t.update != null)
}

/** One update rendered inline: summary, its labelled sections, and a status line.
 *  The shared shape used both per-ticket (in a target row) and for the whole draft.
 *
 *  `boxed` gives it a surface of its own, titled and with the lead paragraph ruled
 *  off from the sections. WITHOUT IT this was a stack of headings and bullets
 *  running flush into the matched-ticket row above and the provenance line below,
 *  so the one thing on the screen that will actually be posted read as more
 *  commentary about the match - a document with no edges and no name. Left off
 *  inside a target row, where the row is already the frame and a second one would
 *  nest. */
export function UpdateBody({ update, boxed = false, unframed = false }: {
  update: GeneratedWorklogUpdate
  boxed?: boolean
  /** `boxed` typography without the frame: `DraftDocument` supplies the card and
   *  the header, and a second border inside it would nest. */
  unframed?: boolean
}) {
  const sections = update.sections.filter((s) => s.heading.trim() && s.points.some((p) => p.trim()))
  const body = (
    <>
      {update.summary && (
        <p className="mt-body-sm" style={{
          color: 'var(--t-title)',
          fontSize: boxed ? 13.5 : 13, lineHeight: 1.6,
          fontWeight: boxed ? 500 : 400,
        }}>
          {update.summary}
        </p>
      )}
      {sections.length > 0 && (
        <div className={boxed ? 'space-y-3.5 mt-3.5 pt-3.5 border-t' : 'space-y-2.5 mt-2.5'}
          style={boxed ? { borderColor: 'var(--t-hair)' } : undefined}>
          {/* BOXED IS READ, NOT SCANNED, so it does not use the `Field`/`Bullets`
              kit the rest of the panel does. That kit is tuned for metadata beside
              a card - an 11px uppercase kicker in `--t-faint` over 12px `--t-muted`
              lines - and at those sizes and contrasts a paragraph of real prose
              reads as blurred rather than quiet. This is the text that gets posted
              on someone's ticket, so it is set as body copy: solid `--t-title` at
              13px, and headings that are legible without being louder than the
              sentences under them. */}
          {sections.map((sec, i) => boxed ? (
            <div key={`${sec.heading}-${i}`}>
              <p className="mt-label mb-1.5" style={{ color: 'var(--t-muted)' }}>{sec.heading}</p>
              <ul className="space-y-1.5">
                {sec.points.filter((p) => p.trim()).map((p, j) => (
                  <li key={j} className="flex gap-2.5"
                    style={{ color: 'var(--t-title)', fontSize: 13, lineHeight: 1.6, fontWeight: 400 }}>
                    <span aria-hidden className="shrink-0" style={{ color: 'var(--t-faint-2)' }}>·</span>
                    <span className="flex-1 min-w-0">{p}</span>
                  </li>
                ))}
              </ul>
            </div>
          ) : (
            <Field key={`${sec.heading}-${i}`} label={sec.heading}>
              <Bullets items={sec.points.filter((p) => p.trim())} size={12} />
            </Field>
          ))}
        </div>
      )}
      {update.status && (
        // A STATE, not another bulleted section. Rendered as a labelled chip on
        // its own ruled line so it reads as the verdict on everything above it,
        // which is what a reader of the ticket will take from it.
        <div className={boxed ? 'flex items-center gap-2 mt-3.5 pt-3 border-t' : 'flex items-center gap-2 mt-2.5'}
          style={boxed ? { borderColor: 'var(--t-hair)' } : undefined}>
          <span className="mt-label" style={{ color: boxed ? 'var(--t-muted)' : 'var(--t-faint)' }}>STATUS</span>
          <span className="rounded-md px-2 py-0.5" style={{
            fontSize: 11.5, fontWeight: 700, color: 'var(--color-state-approved)',
            background: 'color-mix(in srgb, var(--color-state-approved) 12%, transparent)',
          }}>
            {update.status}
          </span>
        </div>
      )}
    </>
  )

  if (!boxed) return <div className="mt-1.5">{body}</div>
  if (unframed) return body

  return (
    <section className="rounded-xl" style={{
      background: 'var(--t-box)', border: '1px solid var(--t-card-border)',
    }}>
      {/* ONE LINE. It used to carry a second, "Posted on the ticket exactly as
          written here" - written to answer the question a draft raises, "is this
          the thing that gets posted or a note to me about it?". The box answers
          that on its own now that it is titled and framed, and the sentence was
          costing more than it earned: a third grey size stacked above the prose,
          which is exactly what made this corner read as mush. */}
      <header className="flex items-center justify-between gap-3 px-4 py-3 border-b"
        style={{ borderColor: 'var(--t-hair)' }}>
        <p className="mt-label" style={{ color: 'var(--t-muted)' }}>THE UPDATE</p>
        <CopyUpdate update={update} />
      </header>
      <div className="px-4 py-3.5">{body}</div>
    </section>
  )
}

/** The update as plain text, in the order it is rendered.
 *
 *  Exported for its unit test - the shape matters more than it looks, because
 *  this is what lands in a standup message or a PR description, and a version
 *  that dropped the section headings would read as one undifferentiated wall.
 *  Markdown-ish rather than the ticket's own markup: the destination is unknown
 *  (Slack, a commit message, an email), and `##` degrades to something legible
 *  everywhere while a tracker's wiki syntax does not. */
export function updateToText(update: GeneratedWorklogUpdate): string {
  const parts: string[] = []
  if (update.summary.trim()) parts.push(update.summary.trim())
  for (const sec of update.sections) {
    const points = sec.points.filter((p) => p.trim())
    if (!sec.heading.trim() || points.length === 0) continue
    parts.push([`## ${sec.heading.trim()}`, ...points.map((p) => `- ${p.trim()}`)].join('\n'))
  }
  if (update.status.trim()) parts.push(`Status: ${update.status.trim()}`)
  return parts.join('\n\n')
}

/** Copy the whole update to the clipboard.
 *
 *  The draft is often wanted somewhere the tracker is not - pasted into a
 *  standup, a PR description, a message to whoever asked. Without this the only
 *  way out of the box was to select prose that spans headings and bullets by
 *  hand, which picks up the labels as body text. */
function CopyUpdate({ update }: { update: GeneratedWorklogUpdate }) {
  const [done, setDone] = useState(false)
  const copy = () => {
    navigator.clipboard.writeText(updateToText(update))
      .then(() => {
        setDone(true)
        setTimeout(() => setDone(false), 1800)
      })
      // Silent: a clipboard the browser refused is not something the user can
      // act on, and an error toast over a draft they were reading is worse than
      // the button appearing not to have fired.
      .catch(() => {})
  }
  return (
    <button onClick={copy} title="Copy this update"
      className="mt-body-sm inline-flex items-center gap-1.5 rounded-lg px-2.5 py-1.5 shrink-0"
      style={{
        fontSize: 11.5, fontWeight: 700, cursor: 'pointer',
        color: done ? 'var(--color-state-approved)' : 'var(--t-muted)',
        border: `1px solid ${done ? 'color-mix(in srgb, var(--color-state-approved) 38%, transparent)' : 'var(--t-hair)'}`,
        background: 'transparent',
      }}>
      <span aria-hidden>{done ? '✓' : '⧉'}</span> {done ? 'Copied' : 'Copy'}
    </button>
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
