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

export function IntegrationsSection({ integrations, onChanged }: {
  integrations: IntegrationsResponse | null
  onChanged: () => void
}) {
  const connected = integrations
    ? TRACKERS.filter(t => integrations[t.id])
    : []

  return (
    <div className="max-w-[760px]">
      <p className="mt-label" style={{ color: 'var(--color-state-proposal)' }}>Project management</p>
      <h1 className="mt-title-lg mt-1.5" style={{ color: 'var(--t-title)' }}>Connect your board</h1>
      <p className="mt-body-sm mt-2 max-w-[520px]" style={{ color: 'var(--t-muted)' }}>
        Link the tool your team already uses. Meridian matches each hour of work to the right
        ticket and drafts a work log — you just approve. Connect one to begin; you can switch or
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
        <ConnectTrackers integrations={integrations} onChanged={onChanged} />
      </div>

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

      <p className="mt-body-sm mt-5 text-center" style={{ color: 'var(--t-faint-2)' }}>
        Working solo without a PM tool?{' '}
        <span className="font-bold" style={{ color: 'var(--color-state-proposal)' }}>
          Skip this — Meridian still tracks your day.
        </span>
      </p>
    </div>
  )
}
