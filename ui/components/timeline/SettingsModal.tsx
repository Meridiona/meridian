//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Settings — sidebar + section router, ported from the Claude Design mock
// (Claude Design project b8656e29-ae04-4f69-b17f-d5fab4d00f3a, "Meridian
// Settings") onto the app's real --t-*/--color-state-* tokens and real data.
// Every feature the old flat SettingsView exposed is migrated 1:1 across the
// six sections below — nothing dropped, see each section file's header
// comment for its migration note. `initialSection` lets callers (the
// Toolbar's nav pill "Integrations" item) deep-link straight to a tab.

'use client'

import { useCallback, useEffect, useState } from 'react'
import { load } from '@/lib/bridge'
import type { IntegrationsResponse } from '@/lib/api-types'
import { TRACKERS } from '@/lib/integrations'
import { pendingTaskSync } from '@/lib/taskSync'
import { setLockOutcome, type LockOutcome } from '@/components/tutorial/engine'
import { ModalShell } from './ModalShell'
import { SettingsSidebar } from './settings/SettingsSidebar'
import { IntegrationsSection } from './settings/IntegrationsSection'
import { WorklogSection } from './settings/WorklogSection'
import { IntelligenceSection } from './settings/IntelligenceSection'
import { CaptureSection } from './settings/CaptureSection'
import { NotificationsSection } from './settings/NotificationsSection'
import { AppearanceSection } from './settings/AppearanceSection'
// import { AdvancedSection } from './settings/AdvancedSection'
import { AccountSection } from './settings/AccountSection'
import { useRuntimeSettings } from './settings/useRuntimeSettings'
import { DEFAULT_SETTINGS_SECTION, type SettingsSection } from './settings/types'

/** How long the "you're connected" confirmation holds before the screen moves. It is the
 *  only moment the user is told the interruption is over, so it has to survive being read
 *  once - and it doubles as the beat between finishing here and the planner reappearing,
 *  which otherwise reads as the app jumping. */
const HAND_BACK_MS = 2200

/** The DECLINED hold, which is deliberately shorter. That path has a real
 *  explanation to give - you can still do all of this, here, with AI - and it is
 *  given properly by the walkthrough on a full-screen card once the planner is
 *  back, where it has room. Holding the modal for the full 2.2s first would make
 *  the user read a cramped version of the same reassurance and then read it again,
 *  so this one only has to register as "heard you" before getting out of the way. */
const DECLINE_HAND_BACK_MS = 1100

/** Ceiling on holding the hand-back open for a first ticket sync.
 *
 *  Generous, because covering a real first Jira import here is the entire point - but
 *  finite, because this modal is LOCKED while it waits, and a tracker that never
 *  answers must not be able to strand someone in it. Past this we hand back and the
 *  planner's own wait (`waitForTickets`, 25s) picks up whatever is left. */
const SYNC_HOLD_MAX_MS = 15000

export function SettingsModal({ onClose, initialSection, onReplayTour, onReplayTourFromDay, lock, onLockSatisfied }: {
  onClose: () => void
  initialSection?: SettingsSection
  /** Restart the first-run walkthrough — surfaced in the Account section. */
  onReplayTour: () => void
  onReplayTourFromDay: (() => void) | null
  /** Set when the user was sent here to finish something — connecting an AI engine.
   *
   *  `'soft'` drops the incidental exits (Escape, backdrop, the ×) but keeps a plainly
   *  labelled one. `'required'` removes the exit until the step is actually done, and is
   *  only safe where something else can still get the user out — see the shell, which
   *  reserves it for the walkthrough. Either way this modal owns the release. */
  lock?: 'soft' | 'required'
  /** The required step completed. The caller decides where the user goes next; this
   *  fires once, shortly after the write lands, so the "Connected" state is readable
   *  before the screen moves. */
  onLockSatisfied?: () => void
}) {
  const [section, setSection] = useState<SettingsSection>(initialSection ?? DEFAULT_SETTINGS_SECTION)
  // THE RELEASE. While locked there is no way out of this modal, which is only defensible
  // because the step that closes it is right there and this flips the moment it lands. It
  // is driven by EVIDENCE - a provider written to settings.json and answering, or a tracker
  // that reports connected - never merely by a screen having been visited.
  //
  // HOW the locked step ended, not merely THAT it did. "I don't use a project tool"
  // is a completed step - the user answered the question - but it is a different
  // answer, and the line shown on the way out has to match it or the app claims a
  // connection that does not exist.
  const [outcome, setOutcome] = useState<LockOutcome | null>(null)
  const connected = outcome !== null
  const required = lock === 'required' && !connected

  /** Release the lock once, recording how. Also parked on the tour's module flag,
   *  which is the only way the script can learn the branch: by the time it looks,
   *  the modal that knew the answer has closed. */
  const release = useCallback((how: LockOutcome) => {
    setOutcome((prev) => prev ?? how)
    setLockOutcome(how)
  }, [])
  const shellLock = lock
    ? {
        label: connected ? 'Done' : lock === 'required' ? 'Connect one to continue' : "I'll do this later",
        required,
      }
    : undefined

  /** True while the hand-back is being held open for a first ticket sync. Drives the
   *  confirmation's wording, so the extra seconds are explained rather than felt. */
  const [pullingTickets, setPullingTickets] = useState(false)

  // Hand the flow back on its own. Making the user press Done here would be asking for a
  // click that carries no decision - they just made the only one this screen exists for,
  // and the thing they were doing when it interrupted them is still waiting. The delay is
  // so the confirmation below is READ as an outcome rather than glimpsed as a flicker -
  // long enough for a sentence now, where it used to only have to cover the word "Done".
  //
  // AND, ON A TRACKER CONNECT, IT ALSO ABSORBS THE FIRST SYNC. Connecting a board starts
  // a real round trip to Jira or Linear that can run for tens of seconds. That wait used
  // to be paid on the PLANNER - the user arrived at the screen whose whole purpose is to
  // show their tickets, found it empty, and read "pulling your tickets in" there. The
  // wait is the same length either way; what changes is where it is spent. Here, the
  // screen already says something true and finished, so the seconds read as the last step
  // completing rather than as the destination being broken.
  //
  // Capped, because a stalled sync must never trap someone in a locked modal - past the
  // ceiling we hand back anyway and the planner's own wait covers the remainder.
  useEffect(() => {
    if (!connected || !onLockSatisfied) return
    if (outcome === 'declined') {
      const t = setTimeout(onLockSatisfied, DECLINE_HAND_BACK_MS)
      return () => clearTimeout(t)
    }
    let alive = true
    const after = (ms: number) => new Promise<void>((r) => setTimeout(r, ms))
    // Only integrations pulls tickets; the AI lock has nothing to wait for.
    const pending = section === 'integrations' ? pendingTaskSync() : null
    if (pending) setPullingTickets(true)
    const sync = pending ? Promise.race([pending, after(SYNC_HOLD_MAX_MS)]) : Promise.resolve()
    void Promise.all([after(HAND_BACK_MS), sync]).then(() => { if (alive) onLockSatisfied() })
    return () => { alive = false }
  }, [connected, outcome, section, onLockSatisfied])
  const [integrations, setIntegrations] = useState<IntegrationsResponse | null>(null)
  const [integrationsError, setIntegrationsError] = useState(false)
  const { settings, patch, save } = useRuntimeSettings()

  const fetchIntegrations = () => {
    load<IntegrationsResponse>('/api/integrations', 'get_integrations')
      .then(res => { setIntegrations(res); setIntegrationsError(false) })
      .catch(err => { console.error('Failed to load integrations', err); setIntegrationsError(true) })
  }
  useEffect(fetchIntegrations, [])

  // The INTEGRATIONS lock releases on evidence, exactly like the AI one: a tracker that
  // actually reports connected, read back from `get_integrations` rather than from "the
  // user visited the connect screen". Declining is the other exit, and comes from the
  // section's own opt-out button.
  //
  // `initialSection` is what says which step owns the lock: the tour opens Settings AT
  // the thing it is asking for, so the step it landed on is the step it is holding.
  useEffect(() => {
    if (!lock || initialSection !== 'integrations') return
    if (TRACKERS.some((t) => integrations?.[t.id])) release('connected')
  }, [lock, initialSection, integrations, release])

  return (
    <ModalShell title={lockTitle(lock, initialSection)} onClose={onClose}
      lock={shellLock} maxWidth={980} scrollInside>
      <div className="flex flex-1 min-h-0">
        <SettingsSidebar section={section} onSelect={setSection} integrations={integrations}
          disabled={required} />
        <div className="flex-1 min-w-0 overflow-y-auto nice-scroll px-8 py-7">
          {/* The one thing the user is told before the screen moves under them. Without it
              the modal simply vanishes and the planner reappears, which reads as a glitch -
              the user never learns that the thing they were sent here to do is done, or that
              the app is deliberately putting them back where they were. */}
          {outcome && lock && <ConnectedHandBack outcome={outcome} section={initialSection} pulling={pullingTickets} />}
          {!settings ? (
            <div className="flex flex-col gap-3 max-w-[640px]">
              {[1, 2, 3].map(i => (
                <div key={i} className="rounded-2xl h-28 bg-card" style={{ opacity: 0.5 }} />
              ))}
            </div>
          ) : (
            <>
              {section === 'integrations' && (
                integrationsError && !integrations ? (
                  <div className="flex flex-col items-start gap-2.5">
                    <p className="mt-body-sm" style={{ color: 'var(--status-error-dot)' }}>
                      Couldn't load integrations.
                    </p>
                    <button type="button" onClick={fetchIntegrations}
                      className="mt-body-sm px-3 py-1.5 rounded-md"
                      style={{ border: '1px solid var(--t-hair)', background: 'transparent', cursor: 'pointer' }}>
                      Retry
                    </button>
                  </div>
                ) : (
                  <IntegrationsSection integrations={integrations} onChanged={fetchIntegrations}
                    gate={!!lock && initialSection === 'integrations'}
                    onDecline={() => release('declined')} />
                )
              )}
              {section === 'worklogs' && (
                <WorklogSection settings={settings} patch={patch} save={save} integrations={integrations} />
              )}
              {/* No `patch`: Intelligence stages its pick locally and writes only on Save. */}
              {/* `gate` rides the lock: the modal is locked ONLY when opened by the "Meridian
                  needs an AI engine" path, which is exactly the case where the user has no
                  working provider and is being set up rather than adjusting one. */}
              {section === 'intelligence' && (
                <IntelligenceSection settings={settings} save={save}
                  gate={!!lock && initialSection !== 'integrations'}
                  onConnected={() => release('connected')} />
              )}
              {section === 'capture' && <CaptureSection settings={settings} patch={patch} save={save} />}
              {section === 'notifications' && <NotificationsSection settings={settings} patch={patch} save={save} />}
              {section === 'appearance' && <AppearanceSection />}
              {/* {section === 'advanced' && (
                <AdvancedSection settings={settings} setSettings={setSettings} patch={patch} save={save} />
              )} */}
              {section === 'account' && <AccountSection onReplayTour={onReplayTour} onReplayTourFromDay={onReplayTourFromDay} />}
            </>
          )}
        </div>
      </div>
    </ModalShell>
  )
}

/** What the locked modal is called while it holds the user. Naming the ASK rather than
 *  the app ("Settings") is what makes a modal with no exit legible: the title is the
 *  reason it will not close. It said "Connect your AI engine" for every lock, which was
 *  simply wrong once the tour started locking the tracker step too. */
function lockTitle(lock: 'soft' | 'required' | undefined, section?: SettingsSection): string {
  if (!lock) return 'Settings'
  return section === 'integrations' ? 'Connect your project tool' : 'Connect your AI engine'
}

/** The success line, shown for [`HAND_BACK_MS`] between finishing the step and the
 *  planner coming back.
 *
 *  Two outcomes, two sentences. `declined` is a real answer - "I don't use one" - and
 *  greeting it with "connected" would be the app reporting something that did not
 *  happen, on the one screen whose entire job was to establish whether it had. It also
 *  has to carry the reassurance, because a user who just told us they have no project
 *  tool is entitled to wonder what they have left.
 *
 *  Green only on a real connection: it is Meridian's colour for "connected and
 *  working", so a declined step gets the neutral surface instead. */
function ConnectedHandBack({ outcome, section, pulling }: {
  outcome: LockOutcome
  section?: SettingsSection
  /** The hand-back is being held while the first ticket sync finishes. Names what the
   *  extra seconds are for - the alternative is a confirmation that says "taking you
   *  back" and then visibly does not, which reads as a hang. */
  pulling?: boolean
}) {
  const declined = outcome === 'declined'
  const tracker = section === 'integrations'
  // Short on the declined path ON PURPOSE - the full reassurance is the tour's
  // next beat, on a card with room for it. Two versions of the same paragraph,
  // seconds apart, reads as the app not trusting the user to have heard it.
  const message = declined
    ? 'No problem - taking you back to your tasks…'
    : tracker
      ? pulling
        ? "Great - that's your board connected. Pulling your tickets in…"
        : "Great - that's your board connected. Taking you back to your tasks…"
      : "Great - that's your AI connected. Taking you back to your task…"
  const tone = declined ? 'var(--t-ctrl-border)' : 'var(--color-state-approved)'
  return (
    <div
      role="status"
      // The walkthrough's signal that the lock is DONE. It cannot watch for the
      // modal closing instead: that is a second and a half later, and until then
      // the tour was still saying "pick your tool and sign in" to someone who had
      // just said they do not have one.
      data-tour="lock-handback"
      className="flex items-center rounded-xl mb-5"
      style={{
        gap: 9, padding: '11px 14px',
        background: declined
          ? 'var(--t-box)'
          : 'color-mix(in srgb, var(--color-state-approved) 12%, transparent)',
        border: `1px solid ${tone}`,
        animation: 'mer-handback-in 260ms cubic-bezier(.2,.8,.25,1) both',
      }}>
      <svg width="15" height="15" viewBox="0 0 16 16" fill="none"
        stroke={declined ? 'var(--t-muted)' : 'var(--color-state-approved)'}
        strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className="shrink-0">
        <path d="M3 8.5 6.5 12 13 4.5" />
      </svg>
      <p className="mt-body-sm" style={{ color: 'var(--t-title)', fontWeight: 600 }}>
        {message}
      </p>
      <style>{`
        @keyframes mer-handback-in {
          from { opacity: 0; transform: translateY(-4px) }
          to   { opacity: 1; transform: none }
        }
      `}</style>
    </div>
  )
}
