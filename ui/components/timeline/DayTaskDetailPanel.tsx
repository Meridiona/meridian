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
import { fmtDur, PROVIDER_META, ProviderGlyph } from '@/components/atoms'
import { GeneratingBar } from '@/components/GeneratingBar'
import { load } from '@/lib/bridge'
import { connectedTrackerNames } from '@/lib/integrations'
import type { DayTaskWorklogDraft, IntegrationsResponse } from '@/lib/api-types'
import { clockLabel, clockLabelFromIso, type LaidSegment } from './dayTaskLayout'
import type { SettingsSection } from './settings/types'
import { Bullets, Field, LinkChip } from './dayTaskKit'
import { useWorklog, type WorklogState } from './useWorklog'
import { WorklogTicketPicker } from './WorklogTicketPicker'
import { DraftTargets } from './WorklogTargets'

/** Join tracker names for prose: `['Jira']`→"Jira", `['Jira','Linear']`→"Jira and
 *  Linear", more →"Jira, Linear and GitHub". */
function joinNames(names: string[]): string {
  if (names.length <= 1) return names[0] ?? ''
  return `${names.slice(0, -1).join(', ')} and ${names[names.length - 1]}`
}

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
export function DayTaskDetailPanel({ detail, onClose, onOpenSettings, onOpenTask }: {
  detail: DayTaskDetail
  onClose: () => void
  onOpenSettings: (section?: SettingsSection) => void
  onOpenTask: (key: string, title?: string) => void
}) {
  const { day, id, title, minutes, hue, segments, summary, footLo, footHi } = detail
  const range = segments.length > 0 ? `${clockLabel(footLo)} - ${clockLabel(footHi)}` : ''
  const wl = useWorklog(day, id)

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
  const trackers = connectedTrackerNames(integrations)

  return (
    <div className="dt-detail h-full flex flex-col">
      {/* Scrollable reading content. */}
      <div className="flex-1 min-h-0 overflow-y-auto nice-scroll p-6 space-y-6">
        <Header title={title} minutes={minutes} hue={hue} range={range} onClose={onClose} />

        {segments.length > 0 && (
          <div className="rounded-xl p-4 bg-card" style={{ border: '1px solid var(--t-card-border)' }}>
            <Field label="When"><SegmentList segments={segments} hue={hue} /></Field>
          </div>
        )}

        {summary.length > 0 && (
          <Field label="What was done"><Bullets items={summary} accent={hue} /></Field>
        )}

        {wl.draft && (
          <DraftPreview draft={wl.draft} hue={hue} onOpenTask={onOpenTask}
            busy={wl.phase === 'generating' || wl.phase === 'approving'} onDismiss={wl.dismiss} />
        )}
      </div>

      {/* Pinned action bar — always visible, never scrolls out of reach. */}
      <WorklogFooter wl={wl} hue={hue} linkedTicket={detail.linkedTicket}
        integrations={integrations} trackers={trackers} onOpenSettings={onOpenSettings} />
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

// ── Worklog: draft preview (scrolls) + pinned action footer ──────────────────

/** The generated worklog draft — preview only; the actions live in the footer. */
function DraftPreview({ draft, hue, busy, onOpenTask, onDismiss }: {
  draft: DayTaskWorklogDraft; hue: string; busy: boolean
  onOpenTask: (key: string, title?: string) => void
  onDismiss: (taskKey: string) => void
}) {
  // No link chip here: the tickets are already named twice below — by DraftTargets
  // ("Comment on KAN-12 · 87% match") before you post, and by the footer's
  // "✓ Posted to KAN-12" + its chip after. A third mention beside the heading was
  // just noise repeating the same key down the panel.
  const generatedAt = clockLabelFromIso(draft.updated_at)
  return (
    <div>
      <div className="flex items-center justify-between gap-2 mb-2.5">
        <p className="mt-label" style={{ color: hue, fontWeight: 700 }}>Worklog draft</p>
        {generatedAt && (
          <p className="text-[10.5px]" style={{ color: 'var(--t-faint)' }}>
            Generated at {generatedAt} · still working on this? Regenerate below
          </p>
        )}
      </div>
      <div className="rounded-xl p-4 space-y-3"
        style={{ border: `1px solid color-mix(in srgb, ${hue} 26%, transparent)`, background: `color-mix(in srgb, ${hue} 5%, var(--t-card))` }}>
        <DraftTargets draft={draft} busy={busy} onOpenTask={onOpenTask} onDismiss={onDismiss} />
        {draft.update.summary && (
          <p className="mt-body-sm" style={{ color: 'var(--t-title)', fontSize: 12.5, lineHeight: 1.55 }}>{draft.update.summary}</p>
        )}
        {draft.update.sections
          .filter((sec) => sec.heading.trim() && sec.points.some((p) => p.trim()))
          .map((sec, i) => (
            <Field key={`${sec.heading}-${i}`} label={sec.heading}>
              <Bullets items={sec.points.filter((p) => p.trim())} size={12} />
            </Field>
          ))}
        {draft.update.status && (
          <Field label="Status">
            <p className="mt-body-sm" style={{ color: 'var(--t-muted)', fontSize: 12, lineHeight: 1.5 }}>{draft.update.status}</p>
          </Field>
        )}
      </div>
      <DraftProvenance draft={draft} />
    </div>
  )
}

/** One line under the draft saying what it was actually compared against.
 *
 *  Without this a proposal is silently ambiguous: the user can't tell "your board
 *  has nothing like this" from "this wasn't on today's list", and those call for
 *  completely different reactions. Say which it was, and point at the fix. Stays
 *  quiet once the user has taken over the choice - they know what they picked. */
function DraftProvenance({ draft }: { draft: DayTaskWorklogDraft }) {
  // Once every ticket on the draft is the user's own pick, there is nothing left to
  // explain — they know where they sent it.
  if (draft.targets.length > 0 && draft.targets.every((t) => t.manual)) return null
  const text = draft.propose
    ? 'This work didn\'t match any of today\'s tasks, so Meridian drafted a new one. Only today\'s tasks are compared - if it belongs to another ticket, pick it below.'
    : 'Matched against today\'s tasks only, not your whole board. Remove any that don\'t fit, or pick a different ticket below.'
  return (
    <p className="mt-body-sm mt-2 px-1" style={{ color: 'var(--t-faint)', fontSize: 11.5, lineHeight: 1.5 }}>
      {text}
    </p>
  )
}

/** The pinned footer holding the primary worklog action, with a clear time hint.
 *  The AI match/propose call routes through the user's chosen provider and can
 *  take a while, so we set the expectation both before the click and while it runs. */
function WorklogFooter({ wl, hue, linkedTicket, integrations, trackers, onOpenSettings }: {
  wl: WorklogState; hue: string; linkedTicket: string | null
  integrations: IntegrationsResponse | null
  trackers: string[]
  onOpenSettings: (section?: SettingsSection) => void
}) {
  const { draft, phase, error, posted, confirming, setConfirming, generate, approve, retarget } = wl
  const busy = phase === 'generating' || phase === 'approving'
  const done = draft?.targets.filter((t) => t.posted) ?? []
  // Integrations loaded and nothing connected → the feature can't match/post, so
  // prompt to connect a tracker instead of offering a dead Generate.
  const noTracker = integrations !== null && trackers.length === 0

  // The manual ticket picker. Local to the footer and reset whenever the draft's
  // targets change, so a successful pick closes it without the caller wiring that up.
  const [picking, setPicking] = useState(false)
  const targetKeys = draft?.targets.map((t) => t.task_key).join(',')
  useEffect(() => { setPicking(false) }, [targetKeys, draft?.propose?.title])

  return (
    <div className="shrink-0 p-4 space-y-2.5"
      style={{ background: 'var(--t-card)', borderTop: '1px solid var(--t-card-border)', boxShadow: '0 -10px 26px -18px rgba(0,0,0,0.35)' }}>
      {error && (
        <p className="mt-body-sm rounded-lg px-3 py-2"
          style={{ color: 'var(--color-state-pending)', background: 'color-mix(in srgb, var(--color-state-pending) 12%, transparent)', fontSize: 12 }}>
          {error}
        </p>
      )}

      {phase === 'generating' ? (
        <GeneratingBar hue={hue} label="Generating your worklog…"
          detail="Reading your work, comparing it against today's tasks and drafting the update - you can keep using Meridian while this runs." />
      ) : posted ? (
        <PostedBar done={done} linkedTicket={linkedTicket} provider={draft?.provider ?? ''} />
      ) : !draft ? (
        noTracker
          ? <ConnectTrackerCta hue={hue} onConnect={() => onOpenSettings('integrations')} />
          : <GenerateCta hue={hue} trackers={trackers} disabled={busy || phase === 'loading'} onGenerate={generate} />
      ) : picking ? (
        <WorklogTicketPicker current={draft.targets[0]?.task_key ?? null} busy={busy}
          onPick={retarget} onCancel={() => setPicking(false)} />
      ) : confirming ? (
        <ConfirmPost draft={draft} busy={busy} approving={phase === 'approving'} onApprove={approve} onCancel={() => setConfirming(false)} />
      ) : (
        <DraftActions draft={draft} hue={hue} busy={busy} onApprove={() => setConfirming(true)}
          onRegenerate={generate} onPick={() => setPicking(true)} />
      )}
    </div>
  )
}

/** Everything posted — one link chip per ticket that took the update.
 *
 *  Lists every ticket rather than the day-task's `linked_ticket`: that column holds
 *  one key, so a two-ticket update would silently report half of what it did. The
 *  linked ticket is only the fallback for a row posted before the draft existed. */
function PostedBar({ done, linkedTicket, provider }: { done: PostedLink[]; linkedTicket: string | null; provider: string }) {
  const links: PostedLink[] = done.length > 0
    ? done
    : linkedTicket ? [{ task_key: linkedTicket, browse_url: null, provider }] : []
  // All of a draft's targets share one tracker, so the first link's provider (or
  // the draft's own, for the linkedTicket-only fallback with no targets at all)
  // is the one to badge.
  const meta = PROVIDER_META[links[0]?.provider ?? provider]
  return (
    <div className="space-y-1.5">
      <span className="inline-flex items-center gap-1.5 rounded-full py-1 pl-1 pr-3"
        style={{ background: `color-mix(in srgb, ${meta?.color ?? 'var(--color-state-approved)'} 14%, transparent)` }}>
        <ProviderGlyph provider={links[0]?.provider ?? provider} size={18} />
        <span className="mt-body-sm" style={{ color: meta?.color ?? 'var(--color-state-approved)', fontSize: 12.5, fontWeight: 700 }}>
          Posted{links.length > 1 ? ` to ${links.length} tickets` : links[0] ? ` to ${links[0].task_key}` : ''}
        </span>
      </span>
      {links.length > 0 && (
        <div className="flex flex-wrap items-center gap-1.5">
          {links.map((l) => <LinkChip key={l.task_key} label={l.task_key} url={l.browse_url} />)}
        </div>
      )}
    </div>
  )
}

/** The slice of a posted target [`PostedBar`] links to. */
interface PostedLink { task_key: string; browse_url: string | null; provider: string }

/** No draft yet, a tracker connected — the primary generate CTA.
 *
 *  The copy says OUT LOUD that only today's tasks are compared against. That is
 *  not a detail: the user picked those tasks this morning, and if they don't know
 *  that's the whole comparison set, a proposal for unplanned work reads as
 *  Meridian failing to find an obvious match rather than doing what it said. The
 *  second sentence is the release valve, so the limit never feels like a wall.
 *
 *  No time claim here - that's shown once it is actually running (GeneratingBar). */
function GenerateCta({ hue, trackers, disabled, onGenerate }: {
  hue: string; trackers: string[]; disabled: boolean; onGenerate: () => void
}) {
  const where = trackers.length > 0 ? joinNames(trackers) : 'your tracker'
  return (
    <div>
      <button onClick={onGenerate} disabled={disabled}
        className="w-full inline-flex items-center justify-center gap-2 rounded-xl px-4 py-3"
        style={{ fontSize: 14, fontWeight: 700, color: '#fff', background: hue, opacity: disabled ? 0.55 : 1, cursor: disabled ? 'default' : 'pointer', boxShadow: `0 8px 22px -10px ${hue}` }}>
        <span aria-hidden>✨</span> Generate worklog
      </button>
      <p className="mt-2.5 text-center" style={{ color: 'var(--t-muted)', fontSize: 13, lineHeight: 1.55 }}>
        Meridian checks this work against <span style={{ fontWeight: 700, color: 'var(--t-title)' }}>today&apos;s tasks only</span> - not your whole board - and writes a short status update. If it doesn&apos;t belong to any of them, it proposes a new {where} issue instead, and you can pick a different ticket yourself. Nothing posts until you approve it.
      </p>
    </div>
  )
}

/** No draft yet, and no tracker connected — the feature can't match or post, so
 *  invite the user to connect a PM app instead.
 *
 *  Deliberately no draft on this path: a status update with nowhere to go is a
 *  dead end dressed up as a feature. Lead with what connecting BUYS (drafting,
 *  matching, posting), not with the fact that something is missing. */
function ConnectTrackerCta({ hue, onConnect }: { hue: string; onConnect: () => void }) {
  return (
    <div>
      <button onClick={onConnect}
        className="w-full inline-flex items-center justify-center gap-2 rounded-xl px-4 py-3"
        style={{ fontSize: 14, fontWeight: 700, color: '#fff', background: hue, cursor: 'pointer', boxShadow: `0 8px 22px -10px ${hue}` }}>
        <span aria-hidden>🔗</span> Connect a tracker to auto-log this
      </button>
      <p className="mt-2.5 text-center" style={{ color: 'var(--t-muted)', fontSize: 13, lineHeight: 1.55 }}>
        Connect Jira, Linear, GitHub, Trello, or Azure DevOps and Meridian drafts this work into a status update, matches it to the right issue, and posts it for you - so your tickets stay current without you writing them up. You approve every post.
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
function DraftActions({ draft, hue, busy, onApprove, onRegenerate, onPick }: {
  draft: DayTaskWorklogDraft; hue: string; busy: boolean
  onApprove: () => void; onRegenerate: () => void; onPick: () => void
}) {
  // Nothing to post to (every match dismissed, no proposal) — approve would only
  // fail, so don't offer it. The picker below is the way out.
  const nowhere = !draft.propose && draft.targets.length === 0
  return (
    <div className="space-y-2">
      <div className="flex items-center gap-2">
        <button onClick={onApprove} disabled={busy || nowhere}
          className="mt-body-sm flex-1 inline-flex items-center justify-center gap-1.5 rounded-xl px-4 py-2.5"
          style={{ fontWeight: 700, color: '#fff', background: hue, opacity: busy || nowhere ? 0.55 : 1, cursor: busy || nowhere ? 'default' : 'pointer', boxShadow: `0 8px 22px -10px ${hue}` }}>
          {draft.propose ? 'Create & post' : draft.targets.length > 1 ? `Approve & post to ${draft.targets.length}` : 'Approve & post'}
        </button>
        <button onClick={onRegenerate} disabled={busy}
          className="mt-body-sm inline-flex items-center gap-1.5 rounded-xl px-4 py-2.5"
          style={{ color: 'var(--t-muted)', border: '1px solid var(--t-hair)', opacity: busy ? 0.55 : 1, cursor: busy ? 'default' : 'pointer' }}
          title="Regenerate - overwrites this draft">
          <span aria-hidden>↻</span> Regenerate
        </button>
      </div>
      <button onClick={onPick} disabled={busy}
        className="mt-body-sm w-full rounded-lg px-3 py-2"
        style={{ color: 'var(--t-muted)', border: '1px solid var(--t-hair)', fontSize: 12.5, opacity: busy ? 0.55 : 1, cursor: busy ? 'default' : 'pointer' }}>
        {draft.propose || draft.targets.length === 0
          ? 'Match to one of my tickets instead'
          : draft.targets.length > 1
            ? 'Post to just one ticket instead'
            : 'Match to a different ticket'}
      </button>
    </div>
  )
}

/** Draft ready, user is confirming the post. */
function ConfirmPost({ draft, busy, approving, onApprove, onCancel }: {
  draft: DayTaskWorklogDraft; busy: boolean; approving: boolean; onApprove: () => void; onCancel: () => void
}) {
  // Name every ticket, not a count: this is the last screen before a comment goes
  // on someone else's board, and "post to 3 tickets?" is not something you can
  // meaningfully say yes to.
  const where = draft.targets.map((t) => t.task_key).join(', ')
  return (
    <div className="space-y-2">
      <p className="mt-body-sm text-center" style={{ color: 'var(--t-muted)', fontSize: 12.5 }}>
        {draft.propose
          ? `Create a new ${draft.propose.issue_type} and post this update?`
          : `Post this update to ${where || 'the tracker'}?`}
      </p>
      <div className="flex items-center gap-2">
        <button onClick={onApprove} disabled={busy}
          className="mt-body-sm flex-1 rounded-xl px-4 py-2.5"
          style={{ fontWeight: 700, color: '#fff', background: 'var(--color-state-approved)', opacity: busy ? 0.55 : 1, cursor: busy ? 'default' : 'pointer' }}>
          {approving ? 'Posting…' : 'Yes, post'}
        </button>
        <button onClick={onCancel} disabled={busy}
          className="mt-body-sm rounded-xl px-4 py-2.5"
          style={{ color: 'var(--t-faint)', border: '1px solid var(--t-hair)', opacity: busy ? 0.55 : 1, cursor: busy ? 'default' : 'pointer' }}>
          Cancel
        </button>
      </div>
    </div>
  )
}

