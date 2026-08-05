//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The worklog's ACTION surfaces - one per state of the draft machine: nothing yet
// (generate, or connect a tracker first), a draft ready (approve / regenerate /
// re-match), confirming a post, and posted.
//
// Split out of `DayTaskDetailPanel` when the worklog moved into its own dialog.
// They are only ever rendered inside that dialog's footer; the panel itself now
// shows a one-line status and a way in (see `WorklogEntry`).
//
// # Who calls this
// [`WorklogDraftDialog`], from its pinned footer.
//
// # Related
// - `./WorklogDraftDialog.tsx` — the dialog these live in
// - `./useWorklog.ts` — the state machine whose phases these render

import { ProviderIcon } from '@/components/ProviderIcon'
import { openExternal } from '@/lib/bridge'
import { availableTrackerNames, trackerName as providerName } from '@/lib/integrations'
import type { DayTaskWorklogDraft } from '@/lib/api-types'

/** Join tracker names for prose: `['Jira']`→"Jira", `['Jira','Linear']`→"Jira and
 *  Linear", more →"Jira, Linear and GitHub". */
export function joinNames(names: string[]): string {
  if (names.length <= 1) return names[0] ?? ''
  return `${names.slice(0, -1).join(', ')} and ${names[names.length - 1]}`
}

/** The slice of a posted target [`PostedBar`] links to. */
export interface PostedLink { task_key: string; browse_url: string | null; provider: string }

/** Everything posted — the outcome, then one clickable ticket per post.
 *
 *  Lists every ticket rather than the day-task's `linked_ticket`: that column holds
 *  one key, so a two-ticket update would silently report half of what it did. The
 *  linked ticket is only the fallback for a row posted before the draft existed. */
export function PostedBar({ done, linkedTicket, provider, hue, busy, onRegenerate }: {
  done: PostedLink[]; linkedTicket: string | null; provider: string
  hue: string; busy: boolean; onRegenerate: () => void
}) {
  const links: PostedLink[] = done.length > 0
    ? done
    : linkedTicket ? [{ task_key: linkedTicket, browse_url: null, provider }] : []
  const ok = 'var(--color-state-approved)'
  // SAID ONCE. This used to print a pill reading "Posted to MER-475" and then,
  // immediately beside it, a chip reading "Linked to MER-475" - the same fact in
  // two shapes with two verbs, which reads as two different things having
  // happened and leaves the user working out which one to click. The outcome is
  // stated once, and the ticket itself is the link.
  return (
    <div data-tour="wl-posted" className="flex items-center justify-between gap-4">
      <div className="flex items-center gap-2 min-w-0 flex-wrap">
        <span className="inline-flex items-center gap-1.5 shrink-0">
          <span aria-hidden className="inline-flex items-center justify-center rounded-full"
            style={{ width: 17, height: 17, background: ok, color: '#fff', fontSize: 10.5, fontWeight: 700 }}>✓</span>
          <span className="mt-body-sm" style={{ color: ok, fontSize: 12.5, fontWeight: 700 }}>
            Posted{links.length === 0 ? '' : links.length > 1 ? ` to ${links.length} tickets` : ' to'}
          </span>
        </span>
        {links.map((l) => <TicketLink key={l.task_key} link={l} fallbackProvider={provider} />)}
      </div>
      {/* Posting appends a comment and can't be undone, but work keeps going - so
          the posted update goes stale. Regenerate drafts a fresh follow-up from the
          work done since; approving it posts a NEW comment (the one already on the
          tracker stays). Kept quiet (a text link, not a button) so it never reads as
          "your post didn't land". */}
      <button onClick={onRegenerate} disabled={busy}
        className="mt-body-sm inline-flex items-center gap-1.5 rounded-lg px-3 py-2 shrink-0"
        style={{ color: 'var(--t-muted)', border: '1px solid var(--t-hair)', fontSize: 12.5, opacity: busy ? 0.55 : 1, cursor: busy ? 'default' : 'pointer' }}
        title="Regenerate a fresh update and post it as a follow-up comment">
        <span aria-hidden>↻</span> Draft a follow-up
      </button>
    </div>
  )
}

/** One posted ticket, as the thing you click to go and read it.
 *
 *  The tracker's own mark rather than the word "Jira": recognised faster than it
 *  is read, and it keeps the chip to a glance. Non-clickable when the provider
 *  gave us no browse URL - still shown, because "which ticket" is the useful part
 *  and a missing link is not a reason to withhold it. */
function TicketLink({ link, fallbackProvider }: { link: PostedLink; fallbackProvider: string }) {
  const url = link.browse_url
  return (
    <button onClick={() => url && openExternal(url)}
      disabled={!url}
      title={url ? 'Open on your tracker' : 'No link available for this ticket'}
      className="inline-flex items-center gap-1.5 rounded-lg px-2 py-1 shrink-0"
      style={{
        border: '1px solid color-mix(in srgb, var(--color-state-approved) 38%, transparent)',
        background: 'color-mix(in srgb, var(--color-state-approved) 8%, transparent)',
        cursor: url ? 'pointer' : 'default',
      }}>
      <ProviderIcon provider={link.provider || fallbackProvider} size={12} />
      <span style={{ color: 'var(--color-state-approved)', fontSize: 11.5, fontWeight: 700, fontFamily: 'var(--font-jetbrains-mono), monospace' }}>
        {link.task_key}
      </span>
      {url && <span aria-hidden style={{ color: 'var(--color-state-approved)', fontSize: 10, opacity: 0.8 }}>↗</span>}
    </button>
  )
}

/** No draft yet, a tracker connected — the primary generate CTA.
 *
 *  The copy says OUT LOUD that only today's tasks are compared against. That is
 *  not a detail: the user picked those tasks this morning, and if they don't know
 *  that's the whole comparison set, a proposal for unplanned work reads as
 *  Meridian failing to find an obvious match rather than doing what it said. The
 *  second sentence is the release valve, so the limit never feels like a wall.
 *
 *  No time claim here - that's shown once it is actually running (GeneratingBar). */
export function GenerateCta({ hue, trackers, disabled, checking, blocked, onGenerate, onConnectProvider }: {
  hue: string; trackers: string[]; disabled: boolean
  /** A health read is in flight from the last press. */
  checking: boolean
  /** That read came back: there is no working provider to write this with. */
  blocked: boolean
  onGenerate: () => void
  onConnectProvider: () => void
}) {
  const where = trackers.length > 0 ? joinNames(trackers) : 'your tracker'
  return (
    <div>
      <button data-tour="wl-generate" onClick={onGenerate} disabled={disabled}
        className="w-full inline-flex items-center justify-center gap-2 rounded-xl px-4 py-3"
        style={{ fontSize: 14, fontWeight: 700, color: '#fff', background: hue, opacity: disabled ? 0.55 : 1, cursor: disabled ? 'default' : 'pointer', boxShadow: `0 8px 22px -10px ${hue}` }}>
        <span aria-hidden>✨</span> {checking ? 'Checking…' : 'Generate worklog'}
      </button>

      {/* THE ANSWER TO THE PRESS, not a pre-emptive replacement of it. Appears
          under the button the user just used, so the reason is attached to the
          action that provoked it and the button stays where it was. */}
      {blocked ? (
        <div className="rounded-xl p-3.5 mt-2.5" style={{
          background: 'color-mix(in srgb, var(--color-state-pending) 10%, transparent)',
          border: '1px solid color-mix(in srgb, var(--color-state-pending) 30%, transparent)',
        }}>
          <p className="mt-body-sm" style={{ color: 'var(--t-title)', fontSize: 13, lineHeight: 1.55 }}>
            Connect an AI provider to generate worklogs. Meridian needs a model to turn
            this work into an update - the one you picked isn&apos;t answering.
          </p>
          <button onClick={onConnectProvider}
            className="mt-body-sm rounded-lg px-3 py-1.5 mt-2.5"
            style={{ fontWeight: 700, fontSize: 12.5, color: '#fff', background: hue, border: 'none', cursor: 'pointer' }}>
            Choose a provider
          </button>
        </div>
      ) : (
        <p className="mt-2.5 text-center" style={{ color: 'var(--t-muted)', fontSize: 13, lineHeight: 1.55 }}>
          Meridian checks this work against <span style={{ fontWeight: 700, color: 'var(--t-title)' }}>today&apos;s tasks only</span> - not your whole board - and writes a short status update. If it doesn&apos;t belong to any of them, it proposes a new {where} issue instead, and you can pick a different ticket yourself. Nothing posts until you approve it.
        </p>
      )}
    </div>
  )
}

/** No draft yet, and no tracker connected — the feature can't match or post, so
 *  invite the user to connect a PM app instead.
 *
 *  Deliberately no draft on this path: a status update with nowhere to go is a
 *  dead end dressed up as a feature. Lead with what connecting BUYS (drafting,
 *  matching, posting), not with the fact that something is missing. */
export function ConnectTrackerCta({ hue, onConnect }: { hue: string; onConnect: () => void }) {
  return (
    <div>
      <button onClick={onConnect}
        className="w-full inline-flex items-center justify-center gap-2 rounded-xl px-4 py-3"
        style={{ fontSize: 14, fontWeight: 700, color: '#fff', background: hue, cursor: 'pointer', boxShadow: `0 8px 22px -10px ${hue}` }}>
        <span aria-hidden>🔗</span> Connect a tracker to auto-log this
      </button>
      <p className="mt-2.5 text-center" style={{ color: 'var(--t-muted)', fontSize: 13, lineHeight: 1.55 }}>
        {/* Derived, never a written-out list. This named all five trackers in the
            registry - including the three flagged `comingSoon` - on a CTA whose
            whole job is to promise a payoff. Someone who uses Trello read it as
            an offer and found the row greyed out. */}
        Connect {availableTrackerNames()} and Meridian drafts this work into a status update, matches it to the right issue, and posts it for you - so your tickets stay current without you writing them up. You approve every post.
      </p>
    </div>
  )
}

/** Draft ready — approve (primary), regenerate (overwrites), or pick the ticket
 *  yourself. Clicking Regenerate flips the footer straight to the GeneratingBar,
 *  so that control itself never needs an in-progress label.
 *
 *  The pick affordance is ALWAYS offered, not just when nothing matched. Meridian
 *  only compares against today's planned tasks, so it can be confidently wrong
 *  about a day that went off-plan, and there'd be nothing the user could do about
 *  it. The wording changes with the draft, since "a different ticket" is
 *  nonsense when no ticket was chosen. Picking replaces every matched ticket with
 *  the one chosen; to drop just one of several, use the ✕ on its row. */
export function DraftActions({ draft, hue, busy, onApprove, onPick }: {
  draft: DayTaskWorklogDraft; hue: string; busy: boolean
  onApprove: () => void; onPick: () => void
}) {
  // Nothing to post to (every match dismissed, no proposal) — approve would only
  // fail, so don't offer it. The picker below is the way out.
  const nowhere = !draft.propose && draft.targets.length === 0
  return (
    // TWO CONTROLS, ONE DECISION. There were three bordered buttons in this row -
    // Regenerate, the ticket picker, and the primary - and the user read all
    // three every time, because three equally-framed boxes look like three
    // options of the same kind. They are not: two answer "where does this go",
    // and Regenerate answers "is this text right", which is a question about the
    // document. So it moved onto the document (see `RegenerateDraft`) and this
    // row is now the one decision it always was, secondary beside primary.
    <div className="flex items-center gap-2">
      <button data-tour="wl-rematch" onClick={onPick} disabled={busy}
        className="mt-body-sm rounded-xl px-4 py-2.5 shrink-0"
        style={{ color: 'var(--t-title)', border: '1px solid var(--t-ctrl-border)', fontSize: 12.5, fontWeight: 600, opacity: busy ? 0.55 : 1, cursor: busy ? 'default' : 'pointer' }}>
        {draft.propose || draft.targets.length === 0
          ? 'Match to my ticket'
          : draft.targets.length > 1
            ? 'Post to one only'
            : 'Change ticket'}
      </button>
      <button data-tour="wl-approve" onClick={onApprove} disabled={busy || nowhere}
        className="mt-body-sm flex-1 inline-flex items-center justify-center gap-1.5 rounded-xl px-4 py-2.5"
        style={{ fontWeight: 700, color: '#fff', background: hue, opacity: busy || nowhere ? 0.55 : 1, cursor: busy || nowhere ? 'default' : 'pointer', boxShadow: `0 8px 22px -10px ${hue}` }}>
        {draft.propose ? 'Create & post' : draft.targets.length > 1 ? `Approve & post to ${draft.targets.length}` : 'Approve & post'}
      </button>
    </div>
  )
}

/** Rewrite the draft — a quiet control on the DOCUMENT, not in the action row.
 *
 *  It used to be the first of three identically-bordered buttons above the
 *  primary, which framed "is this text right?" as one more answer to "where does
 *  this go?". Here it sits beside the line saying when the draft was written,
 *  which is the fact it acts on. Borderless on purpose: it overwrites, so it
 *  should be findable rather than inviting. */
export function RegenerateDraft({ busy, onRegenerate }: { busy: boolean; onRegenerate: () => void }) {
  return (
    <button onClick={onRegenerate} disabled={busy}
      title="Rewrite this update from the work - replaces the current draft"
      className="mt-body-sm inline-flex items-center gap-1.5 rounded-lg px-2.5 py-1.5 shrink-0"
      style={{ color: 'var(--t-muted)', border: 'none', background: 'transparent', fontSize: 11.5, fontWeight: 600, opacity: busy ? 0.45 : 1, cursor: busy ? 'default' : 'pointer' }}>
      <span aria-hidden>↻</span> Rewrite
    </button>
  )
}

/** Draft ready, user is confirming the post. */
export function ConfirmPost({ draft, busy, approving, onApprove, onCancel }: {
  draft: DayTaskWorklogDraft; busy: boolean; approving: boolean; onApprove: () => void; onCancel: () => void
}) {
  // Name every ticket, not a count: this is the last screen before a comment goes
  // on someone else's board, and "post to 3 tickets?" is not something you can
  // meaningfully say yes to.
  const where = draft.targets.map((t) => t.task_key).join(', ')
  return (
    <div className="flex items-center gap-3">
      <p className="mt-body-sm flex-1" style={{ color: 'var(--t-title)', fontSize: 13, lineHeight: 1.5 }}>
        {draft.propose
          // Names the board too: creating a ticket is outward-facing and can't be
          // undone, and with two trackers connected "a new Task" doesn't say where.
          ? `Create a new ${draft.propose.issue_type} in ${providerName(draft.provider)} and post this update?`
          : `Post this update to ${where || 'the tracker'}?`}
      </p>
      <button onClick={onCancel} disabled={busy}
        className="mt-body-sm rounded-xl px-4 py-2.5 shrink-0"
        style={{ color: 'var(--t-faint)', border: '1px solid var(--t-hair)', opacity: busy ? 0.55 : 1, cursor: busy ? 'default' : 'pointer' }}>
        Cancel
      </button>
      <button data-tour="wl-confirm" onClick={onApprove} disabled={busy}
        className="mt-body-sm rounded-xl px-5 py-2.5 shrink-0"
        style={{ fontWeight: 700, color: '#fff', background: 'var(--color-state-approved)', opacity: busy ? 0.55 : 1, cursor: busy ? 'default' : 'pointer' }}>
        {approving ? 'Posting…' : 'Yes, post'}
      </button>
    </div>
  )
}
