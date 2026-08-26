//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Settings → Integrations. Header copy + a connected-summary banner ported
// from the Claude Design mock ("Connect your board"), sitting atop the real,
// already-working connect surface (`ConnectTrackers` / IntegrationConnect.tsx)
// — unchanged, so every existing OAuth/token/Azure-discovery flow keeps
// working exactly as before. The mock's summary banner only ever tracked ONE
// connected provider (its own local demo state); this build supports the
// REAL app, which can have several trackers connected at once, so the
// banner lists every connected provider instead of assuming just one.

'use client'

import ConnectTrackers from '@/components/IntegrationConnect'
import type { IntegrationsResponse } from '@/lib/api-types'
import { TRACKERS } from '@/lib/integrations'

export function IntegrationsSection({ integrations, onChanged, gate = false, onDecline }: {
  integrations: IntegrationsResponse | null
  onChanged: () => void
  /** True when this panel is a REQUIRED step the user was sent to - the walkthrough's
   *  "do you work on a team board?" branch. Adds the opt-out below, and nothing else:
   *  the connect surface itself is identical, because a gated user and a browsing one
   *  are doing exactly the same thing and a second variant of it would drift. */
  gate?: boolean
  /** "I don't use one." Only meaningful under `gate`, where it is the other way out. */
  onDecline?: () => void
}) {
  const connected = integrations
    ? TRACKERS.filter(t => integrations[t.id])
    : []

  return (
    <div className="max-w-[760px]">
      {/* The heading is the BROWSING screen's. Under `gate` the modal header already
          says "Connect your project tool", and stacking "Connect your board" under it
          gave the locked screen two titles for one job - the second reading as a
          separate question the user had to work out the relationship to. Same call
          `IntelligenceSection` makes for its own gate, for the same reason: whoever
          owns the frame owns the title. The paragraph below stays either way, because
          it says what connecting BUYS, which no title does. */}
      {!gate && (
        <>
          <p className="mt-label" style={{ color: 'var(--t-accent)' }}>Project management</p>
          <h1 className="mt-title-lg mt-1.5" style={{ color: 'var(--t-title)' }}>Connect your board</h1>
        </>
      )}
      <p className={`mt-body-sm max-w-[520px] ${gate ? '' : 'mt-2'}`} style={{ color: 'var(--t-muted)' }}>
        Link the tool your team already uses. Meridian matches each hour of work to the right
        ticket and drafts a work log - you just approve. Connect one to begin; you can switch or
        add more anytime.
      </p>

      {connected.length > 0 && (
        <div className="mt-5 flex items-center gap-3 rounded-2xl px-4 py-3.5"
          style={{
            background: 'color-mix(in srgb, var(--color-state-approved) 8%, var(--t-card))',
            border: '1px solid color-mix(in srgb, var(--color-state-approved) 26%, transparent)',
          }}>
          <span className="inline-flex items-center justify-center rounded-lg shrink-0 bg-card"
            style={{ width: 34, height: 34, color: 'var(--color-state-approved)' }} aria-hidden="true">✓</span>
          <div className="min-w-0 flex-1">
            <p className="mt-body-sm font-bold" style={{ color: 'var(--color-state-approved)' }}>
              Connected to {connected.map(t => t.name).join(', ')}
            </p>
            <p className="text-[11.5px] font-semibold mt-0.5" style={{ color: 'color-mix(in srgb, var(--color-state-approved) 75%, var(--t-muted))' }}>
              Syncing every hour
            </p>
          </div>
        </div>
      )}

      <div className="mt-5">
        <ConnectTrackers integrations={integrations} onChanged={onChanged} onDecline={gate ? onDecline : undefined} />
      </div>

      {/* THE WAY OUT. A required step with five providers and no exit assumes every
          user is on one of them - and the person most likely to be stuck here is
          exactly the one this app is easiest to mis-sell to: a solo dev, or someone
          whose team uses something we don't support yet.
          It is a peer of the five tiles, not a footnote under them, and it is on
          screen from the first frame. Discovering an escape hatch only after failing
          an OAuth is discovering it too late - by then the user has already decided
          the app is broken. Meridian genuinely works without a tracker, so declining
          costs them nothing, and saying so plainly here is cheaper than a support
          thread later. */}
      {gate && (
        <div className="mt-5 flex flex-col items-start gap-2 px-4 py-4 rounded-xl"
          style={{ border: '1px dashed var(--t-ctrl-border)' }}>
          <p className="mt-body-sm" style={{ color: 'var(--t-muted)' }}>
            Don&apos;t see yours, or don&apos;t use one? That&apos;s completely fine - Meridian
            works on its own, and you can connect a tool any time from Settings.
          </p>
          <button type="button" data-tour="tracker-decline" onClick={onDecline}
            className="mt-body-sm px-3.5 py-2 rounded-lg mt-card-hover"
            style={{
              color: 'var(--t-title)', fontWeight: 700, cursor: 'pointer',
              background: 'var(--t-box)', border: '1px solid var(--t-ctrl-border)',
            }}>
            I don&apos;t use a project tool
          </button>
        </div>
      )}

      <div className="flex items-center gap-2.5 mt-6 px-3.5 py-3 rounded-xl bg-box">
        <svg width="15" height="17" viewBox="0 0 13 15" fill="none" aria-hidden="true" className="shrink-0">
          <path d="M6.5 1 L12 3.2 V7 C12 10.5 9.5 12.8 6.5 14 C3.5 12.8 1 10.5 1 7 V3.2 Z" stroke="var(--t-faint)" strokeWidth="1.3" strokeLinejoin="round" />
          <path d="M4.4 7.3 L5.9 8.8 L8.7 5.8" stroke="var(--t-faint)" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
        <span className="text-[11.5px] flex-1" style={{ color: 'var(--t-muted)' }}>
          Meridian only ever reads your board and writes logs you approve. We request the
          narrowest scopes each provider allows, and you can revoke access in one click.
        </span>
      </div>

      {/* GONE: a "Working solo without a PM tool? Skip this - Meridian still tracks
          your day." line that lived here. It was a <span> styled as a link - bold,
          accent-coloured, no handler, no href - so clicking it did nothing at all.
          Harmless-looking on the browsable screen; on the walkthrough's LOCKED one
          it was the worst possible bug, because it is precisely the sentence a
          stuck user reaches for, and pressing it and getting nothing is how someone
          decides the modal is broken and they are trapped. The real opt-out above
          says the same thing and works. */}
    </div>
  )
}
