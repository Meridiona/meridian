//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The summary's SECOND view: one strand of the day, and where its update is going.
//
// It replaces the 388px side drawer this screen used to open (`DayTaskDetailPanel`
// in a right-anchored dialog). The drawer was the timeline's surface borrowed
// wholesale, and it brought the timeline's priorities with it: two blocks of
// evidence ABOUT the work first, the draft reachable only through a second dialog on
// top. On the summary the user has already read what the day was - they came here to
// deal with the update - so the document leads and the evidence is gone. It is the
// same arrangement the walkthrough teaches, which is the point: a tour that teaches
// a lookalike has taught the lookalike.
//
// THE WORKLOG HALF IS THE SHIPPED COMPONENTS, not a replica of them. `DraftDocument`,
// `CopyUpdate`, `DraftActions`, `WorklogTicketPicker`, `ConfirmPost`, `PostedBar` and
// `GenerateCta` are the same modules the task panel's draft dialog renders, driven by
// the same `useWorklog` state machine - so generate / approve / retarget / dismiss all
// work here with no worklog logic of its own. The branch ladder below is the one in
// `WorklogDraftDialog`; what differs is the chrome around it, which is the whole
// reason this file exists.
//
// THE POST IS REAL HERE. The walkthrough's version fakes it (its example day never
// happened and its ticket keys are someone else's); this one files the comment.
//
// # Who calls this
// [`DaySummaryOverlay`], when a row in `WorkList` is clicked.
//
// # Related
// - `ui/components/tutorial/TutorialSummaryTask.tsx` — the scripted twin, same shape
// - `ui/components/timeline/WorklogDraftDialog.tsx` — the same ladder, in a dialog
// - `ui/components/timeline/useWorklog.ts` — the state machine behind all of it

'use client'

import { useEffect, useState } from 'react'
import { GeneratingBar } from '@/components/GeneratingBar'
import { fmtDur } from '@/components/atoms'
import { load } from '@/lib/bridge'
import { connectedTrackers } from '@/lib/integrations'
import type { HealthStatus, IntegrationsResponse } from '@/lib/api-types'
import type { DayTaskDetail } from '@/components/timeline/DayTaskDetailPanel'
import { clockLabel } from '@/components/timeline/dayTaskLayout'
import type { SettingsSection } from '@/components/timeline/settings/types'
import { useWorklog } from '@/components/timeline/useWorklog'
import { DraftDocument } from '@/components/timeline/WorklogTargets'
import { CopyUpdate } from '@/components/timeline/WorklogUpdateBody'
import { WorklogTicketPicker } from '@/components/timeline/WorklogTicketPicker'
import {
  ConfirmPost, ConnectTrackerCta, DraftActions, GenerateCta, PostedBar, RegenerateDraft,
  type PostedLink,
} from '@/components/timeline/WorklogActions'

export function SummaryTaskView({ detail, onBack, onOpenSettings, onOpenTask }: {
  detail: DayTaskDetail
  onBack: () => void
  onOpenSettings: (section?: SettingsSection) => void
  onOpenTask: (key: string, title?: string) => void
}) {
  const { day, id, title, minutes, footLo, footHi, segments } = detail
  const wl = useWorklog(day, id)
  const { draft, phase, error, posted, confirming, setConfirming, generate, approve, retarget } = wl
  const busy = phase === 'generating' || phase === 'approving'
  const done: PostedLink[] = draft?.targets.filter(t => t.posted) ?? []
  const hue = 'var(--accent)'
  const range = segments.length > 0 ? `${clockLabel(footLo)} - ${clockLabel(footHi)}` : ''

  // Which PM trackers are connected - names the tracker in the CTA copy, and decides
  // whether to offer Generate or prompt to connect one. `null` while loading so the
  // copy does not flash "connect a tracker" and then swap.
  const [integrations, setIntegrations] = useState<IntegrationsResponse | null>(null)
  useEffect(() => {
    let alive = true
    load<IntegrationsResponse>('/api/integrations', 'get_integrations')
      .then(r => { if (alive) setIntegrations(r) })
      .catch(() => {})
    return () => { alive = false }
  }, [])
  const connected = connectedTrackers(integrations)
  const trackers = connected.map(t => t.name)
  const noTracker = integrations !== null && trackers.length === 0

  // The manual ticket picker, reset whenever the draft's targets change so a
  // successful pick closes it without the caller wiring that up.
  const [picking, setPicking] = useState(false)
  const targetKeys = draft?.targets.map(t => t.task_key).join(',')
  useEffect(() => { setPicking(false) }, [targetKeys, draft?.propose?.title])

  // ── Is there a model to do this AT ALL ─────────────────────────────────────
  // The same pre-flight `WorklogDraftDialog` runs, and for the same reasons: the
  // button keeps its own name either way (so the surface still says what it is FOR),
  // health is re-read on every press (the user may have connected a provider since
  // this opened), and a FAILED probe means proceed - blocking generation because a
  // health read did not come back is strictly worse than the dead end it prevents.
  const [checking, setChecking] = useState(false)
  const [providerDown, setProviderDown] = useState(false)
  const attemptGenerate = async () => {
    setChecking(true)
    try {
      const h = await load<HealthStatus>('/api/health', 'get_health')
      if (h?.llm_provider_ok === false) { setProviderDown(true); return }
      setProviderDown(false)
      generate()
    } catch {
      setProviderDown(false)
      generate()
    } finally {
      setChecking(false)
    }
  }

  return (
    <>
      {/* THE ONLY WAY OUT OF THIS VIEW, so it has to look like a control.
          It was muted body text with no shape, no border and no padding, sitting
          above a heading set at 20px/800 - which is to say the one thing the user
          needs when they are done reading was the quietest thing on the screen
          and, at 15px tall, the smallest thing to aim at. Now: a chip with a real
          hit area, on the control surface everything else clickable uses, in
          title ink. Pulled left by its own padding so the LABEL still aligns with
          the column, which is what the eye reads the margin from. */}
      <button
        onClick={onBack}
        className="mt-body-sm inline-flex items-center gap-1.5 rounded-lg px-2.5 py-1.5 -ml-2.5 bg-ctrl"
        style={{
          color: 'var(--t-title)', border: '1px solid var(--t-ctrl-border)',
          fontWeight: 600, cursor: 'pointer',
        }}
      >
        <span aria-hidden style={{ fontSize: 15, lineHeight: 1, marginTop: -1 }}>‹</span>
        Back to summary
      </button>

      <div className="flex items-start gap-3 mt-4">
        <span
          className="rounded-full shrink-0 mt-1.5"
          style={{ width: 9, height: 9, background: detail.hue }}
          aria-hidden
        />
        <div className="min-w-0">
          <p className="mt-label" style={{ color: 'var(--t-faint-2)' }}>
            WORKLOG UPDATE · {fmtDur(minutes * 60)}{range && ` · ${range}`}
          </p>
          <h2
            className="mt-1.5"
            style={{
              font: '800 20px var(--font-sans)',
              letterSpacing: '-0.024em',
              lineHeight: 1.2,
              color: 'var(--t-title)',
            }}
          >
            {title || 'Activity'}
          </h2>
        </div>
      </div>

      {/* The draft, laid out as the draft dialog lays it out: one frameless document,
          a ticket and the text that goes on it. */}
      <div className="mt-6">
        {draft ? (
          <DraftDocument draft={draft} busy={busy} trackers={connected}
            onOpenTask={onOpenTask} onDismiss={wl.dismiss} onSetProvider={wl.setProvider} />
        ) : phase === 'generating' ? (
          <GeneratingBar hue={hue} label="Generating your worklog…"
            detail="Reading your work, comparing it against today's tasks and drafting the update - you can keep using Meridian while this runs." />
        ) : phase === 'loading' ? null : (
          <EmptyDraft />
        )}
      </div>

      {error && (
        <p className="mt-body-sm mt-4 rounded-lg px-3 py-2"
          style={{
            color: 'var(--color-state-pending)',
            background: 'color-mix(in srgb, var(--color-state-pending) 12%, transparent)',
            fontSize: 12,
          }}>
          {error}
        </p>
      )}

      {/* The controls, in the shipped arrangement: quiet things you can do TO the
          draft above, the decision about where it goes below. */}
      {draft && !posted && !picking && !confirming && phase !== 'generating' && (
        <div className="mt-5 flex items-center gap-1 -ml-1.5">
          <RegenerateDraft busy={busy} onRegenerate={generate} />
          <CopyUpdate update={draft.update} />
        </div>
      )}

      <div className="mt-3">
        {phase === 'generating' ? null
          : posted ? (
            <PostedBar done={done} linkedTicket={detail.linkedTicket} provider={draft?.provider ?? ''}
              hue={hue} busy={busy} onRegenerate={generate} />
          ) : !draft ? (
            noTracker
              ? <ConnectTrackerCta hue={hue} onConnect={() => onOpenSettings('integrations')} />
              : <GenerateCta hue={hue} trackers={trackers} checking={checking}
                  disabled={busy || phase === 'loading' || checking} onGenerate={attemptGenerate}
                  blocked={providerDown} onConnectProvider={() => onOpenSettings('intelligence')} />
          ) : picking ? (
            <WorklogTicketPicker current={draft.targets[0]?.task_key ?? null} busy={busy}
              onPick={retarget} onCancel={() => setPicking(false)} />
          ) : confirming ? (
            <ConfirmPost draft={draft} busy={busy} approving={phase === 'approving'}
              onApprove={approve} onCancel={() => setConfirming(false)} />
          ) : (
            <DraftActions draft={draft} hue={hue} busy={busy}
              onApprove={() => setConfirming(true)} onPick={() => setPicking(true)} />
          )}
      </div>
    </>
  )
}

/** Nothing drafted yet - say what pressing the button will produce, rather than
 *  leaving the document area blank above a call to action. */
function EmptyDraft() {
  return (
    <div className="text-center py-6">
      <p className="mt-body" style={{ color: 'var(--t-title)', fontWeight: 700 }}>
        No update written yet
      </p>
      <p className="mt-2 mx-auto" style={{ color: 'var(--t-muted)', fontSize: 13, lineHeight: 1.6, maxWidth: 380 }}>
        Meridian reads what you actually did in this stretch and writes it up as a
        short status update - the decisions, what you verified, and where it landed.
      </p>
    </div>
  )
}
