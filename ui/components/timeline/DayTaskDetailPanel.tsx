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
import { load } from '@/lib/bridge'
import { connectedTrackerNames } from '@/lib/integrations'
import type { DayTaskWorklogDraft, IntegrationsResponse } from '@/lib/api-types'
import { clockLabel, type LaidSegment } from './dayTaskLayout'
import type { SettingsSection } from './settings/types'
import { Bullets, Field, LinkChip } from './dayTaskKit'
import { useWorklog, type WorklogState } from './useWorklog'

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
export function DayTaskDetailPanel({ detail, onClose, onOpenSettings }: {
  detail: DayTaskDetail
  onClose: () => void
  onOpenSettings: (section?: SettingsSection) => void
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

        {wl.draft && <DraftPreview draft={wl.draft} hue={hue} linkedTicket={detail.linkedTicket} />}
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
function DraftPreview({ draft, hue, linkedTicket }: {
  draft: DayTaskWorklogDraft; hue: string; linkedTicket: string | null
}) {
  const linkKey = draft.target_key || linkedTicket
  return (
    <div>
      <div className="flex items-center justify-between mb-2.5">
        <p className="mt-label" style={{ color: hue, fontWeight: 700 }}>Worklog draft</p>
        {linkKey && <LinkChip label={linkKey} url={draft.browse_url} />}
      </div>
      <div className="rounded-xl p-4 space-y-3"
        style={{ border: `1px solid color-mix(in srgb, ${hue} 26%, transparent)`, background: `color-mix(in srgb, ${hue} 5%, var(--t-card))` }}>
        <DraftBanner draft={draft} />
        {draft.update.summary && (
          <p className="mt-body-sm" style={{ color: 'var(--t-title)', fontSize: 12.5, lineHeight: 1.55 }}>{draft.update.summary}</p>
        )}
        {draft.update.decisions.length > 0 && <Field label="Decisions"><Bullets items={draft.update.decisions} size={12} /></Field>}
        {draft.update.architecture.length > 0 && <Field label="Architecture"><Bullets items={draft.update.architecture} size={12} /></Field>}
        {draft.update.status && (
          <Field label="Status">
            <p className="mt-body-sm" style={{ color: 'var(--t-muted)', fontSize: 12, lineHeight: 1.5 }}>{draft.update.status}</p>
          </Field>
        )}
      </div>
    </div>
  )
}

/** Where the update will land: comment on a matched task, or create a proposed one. */
function DraftBanner({ draft }: { draft: DayTaskWorklogDraft }) {
  if (draft.match) {
    return (
      <div className="rounded-lg px-3 py-2" style={{ background: 'color-mix(in srgb, var(--color-state-proposal) 12%, transparent)' }}>
        <p className="mt-body-sm" style={{ color: 'var(--color-state-proposal)', fontSize: 12, fontWeight: 700 }}>
          Comment on {draft.match.task_key}
          <span style={{ opacity: 0.7, fontWeight: 400 }}> · {Math.round(draft.match.confidence * 100)}% match</span>
        </p>
      </div>
    )
  }
  if (draft.propose) {
    return (
      <div className="rounded-lg px-3 py-2" style={{ background: 'color-mix(in srgb, var(--color-state-pending) 12%, transparent)' }}>
        <p className="mt-body-sm" style={{ color: 'var(--color-state-pending)', fontSize: 12, fontWeight: 700 }}>
          New {draft.propose.issue_type}: {draft.propose.title}
        </p>
        {draft.propose.description && (
          <p className="mt-body-sm mt-1" style={{ color: 'var(--t-muted)', fontSize: 11.5, lineHeight: 1.45 }}>{draft.propose.description}</p>
        )}
      </div>
    )
  }
  return null
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
  const { draft, phase, error, posted, confirming, setConfirming, generate, approve } = wl
  const busy = phase === 'generating' || phase === 'approving'
  const linkKey = draft?.target_key || linkedTicket
  // Integrations loaded and nothing connected → the feature can't match/post, so
  // prompt to connect a tracker instead of offering a dead Generate.
  const noTracker = integrations !== null && trackers.length === 0

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
        <GeneratingBar hue={hue} />
      ) : posted ? (
        <div className="flex items-center justify-between">
          <p className="mt-body-sm inline-flex items-center gap-1.5" style={{ color: 'var(--color-state-approved)', fontSize: 13, fontWeight: 700 }}>
            <span aria-hidden>✓</span> Posted{linkKey ? ` to ${linkKey}` : ''}
          </p>
          {linkKey && <LinkChip label={linkKey} url={draft?.browse_url} />}
        </div>
      ) : !draft ? (
        noTracker
          ? <ConnectTrackerCta hue={hue} onConnect={() => onOpenSettings('integrations')} />
          : <GenerateCta hue={hue} trackers={trackers} disabled={busy || phase === 'loading'} onGenerate={generate} />
      ) : confirming ? (
        <ConfirmPost draft={draft} busy={busy} approving={phase === 'approving'} onApprove={approve} onCancel={() => setConfirming(false)} />
      ) : (
        <DraftActions draft={draft} hue={hue} busy={busy} onApprove={() => setConfirming(true)} onRegenerate={generate} />
      )}
    </div>
  )
}

/** No draft yet, a tracker connected — the primary generate CTA. The copy names
 *  the connected tracker (e.g. Jira) and spells out what will happen, since that
 *  is clearer than a vague "generate". No time claim here — that's shown once it
 *  is actually running (GeneratingBar). */
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
        Meridian finds which <span style={{ fontWeight: 700, color: 'var(--t-title)' }}>{where}</span> issue this work belongs to - or proposes a new one if nothing fits - and writes a short status update. You review it here before anything is posted to {where}.
      </p>
    </div>
  )
}

/** No draft yet, and no tracker connected — the feature can't match or post, so
 *  invite the user to connect a PM app instead. */
function ConnectTrackerCta({ hue, onConnect }: { hue: string; onConnect: () => void }) {
  return (
    <div>
      <button onClick={onConnect}
        className="w-full inline-flex items-center justify-center gap-2 rounded-xl px-4 py-3"
        style={{ fontSize: 14, fontWeight: 700, color: '#fff', background: hue, cursor: 'pointer', boxShadow: `0 8px 22px -10px ${hue}` }}>
        <span aria-hidden>🔗</span> Connect a project tracker
      </button>
      <p className="mt-2.5 text-center" style={{ color: 'var(--t-muted)', fontSize: 13, lineHeight: 1.55 }}>
        Connect Jira, Linear, GitHub, Trello, or Azure DevOps and Meridian will automatically match this work to the right issue and keep your tracker in sync - drafting status updates you approve before they post.
      </p>
    </div>
  )
}

/** Draft ready — approve (primary) or regenerate (overwrites). Clicking
 *  Regenerate flips the footer straight to the GeneratingBar, so this control
 *  itself never needs an in-progress label. */
function DraftActions({ draft, hue, busy, onApprove, onRegenerate }: {
  draft: DayTaskWorklogDraft; hue: string; busy: boolean; onApprove: () => void; onRegenerate: () => void
}) {
  return (
    <div className="flex items-center gap-2">
      <button onClick={onApprove} disabled={busy}
        className="mt-body-sm flex-1 inline-flex items-center justify-center gap-1.5 rounded-xl px-4 py-2.5"
        style={{ fontWeight: 700, color: '#fff', background: hue, opacity: busy ? 0.55 : 1, cursor: busy ? 'default' : 'pointer', boxShadow: `0 8px 22px -10px ${hue}` }}>
        {draft.propose ? 'Create & post' : 'Approve & post'}
      </button>
      <button onClick={onRegenerate} disabled={busy}
        className="mt-body-sm inline-flex items-center gap-1.5 rounded-xl px-4 py-2.5"
        style={{ color: 'var(--t-muted)', border: '1px solid var(--t-hair)', opacity: busy ? 0.55 : 1, cursor: busy ? 'default' : 'pointer' }}
        title="Regenerate - overwrites this draft">
        <span aria-hidden>↻</span> Regenerate
      </button>
    </div>
  )
}

/** Draft ready, user is confirming the post. */
function ConfirmPost({ draft, busy, approving, onApprove, onCancel }: {
  draft: DayTaskWorklogDraft; busy: boolean; approving: boolean; onApprove: () => void; onCancel: () => void
}) {
  return (
    <div className="space-y-2">
      <p className="mt-body-sm text-center" style={{ color: 'var(--t-muted)', fontSize: 12.5 }}>
        {draft.propose ? `Create a new ${draft.propose.issue_type} and post this update?` : `Post this update to ${draft.match?.task_key ?? 'the tracker'}?`}
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

/** The in-progress state: an indeterminate bar + a plain time expectation. */
function GeneratingBar({ hue }: { hue: string }) {
  return (
    <div className="space-y-2">
      <div className="flex items-center justify-between">
        <p className="mt-body-sm inline-flex items-center gap-2" style={{ color: 'var(--t-title)', fontSize: 12.5, fontWeight: 600 }}>
          <span aria-hidden>✨</span> Generating your worklog…
        </p>
        <span className="mt-body-sm animate-pulse" style={{ color: 'var(--t-faint)', fontSize: 11 }}>this might take a minute or so</span>
      </div>
      <div className="rounded-full overflow-hidden" style={{ height: 4, background: 'var(--t-hair)' }}>
        <div className="h-full rounded-full animate-pulse" style={{ width: '55%', background: hue }} />
      </div>
      <p className="mt-body-sm" style={{ color: 'var(--t-faint)', fontSize: 11, lineHeight: 1.5 }}>
        Reading your work, matching it to a task and drafting the update - you can keep using Meridian while this runs.
      </p>
    </div>
  )
}
