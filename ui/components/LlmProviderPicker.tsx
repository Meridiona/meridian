//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The AI-provider picker — ONE component, mounted in both the setup wizard's
// Intelligence step and the dashboard's Settings, exactly as <ConnectTrackers> is shared
// between the wizard and the dashboard. The provider list itself lives in
// `@/lib/llm-providers`; this only renders it and reports the choice up.
//
// Layout is a 2-column card GRID (not a stacked list) so the five providers read as
// comparable options at a glance. Order is the recommendation: coding agents first
// (frontier accuracy), on-device last (private fallback).
//
// It deliberately does NOT save. The wizard and Settings persist differently (the wizard
// writes once per pick; Settings the same through useRuntimeSettings), so the owner does
// the write and this stays a controlled input — one source of truth per surface.

import { useCallback, useEffect, useState } from 'react'
import { invoke, openExternal } from '@/lib/bridge'
import { LLM_PROVIDERS, USAGE_FOOTPRINT_NOTE, type LlmProviderId, type LlmProviderMeta } from '@/lib/llm-providers'

/** One provider's live install state (mirrors `ProviderStatus` in src/llm/detect.rs). */
export interface ProviderStatus {
  id: string
  installed: boolean
  path: string | null
  /** Always null — Meridian reports *installed*, not *signed in*. See src/llm/detect.rs. */
  authenticated: boolean | null
}

/**
 * Probe which provider CLIs exist on this Mac. Re-run on demand (the Rescan button):
 * the user will alt-tab out to `npm i -g …` and come back mid-wizard, and a cached
 * "not installed" would then be a lie.
 */
export function useLlmProviderDetection() {
  const [status, setStatus] = useState<Record<string, ProviderStatus>>({})
  const [scanning, setScanning] = useState(true)

  const rescan = useCallback(async () => {
    setScanning(true)
    try {
      const found = await invoke<ProviderStatus[]>('detect_llm_providers')
      setStatus(Object.fromEntries(found.map((p) => [p.id, p])))
    } catch {
      // A failed probe must not block the step: an un-probed provider renders as
      // "can't tell", and picking it is still allowed (the CLI may well be there).
      setStatus({})
    } finally {
      setScanning(false)
    }
  }, [])

  useEffect(() => { rescan() }, [rescan])
  return { status, scanning, rescan }
}

function ProviderGlyph({ id, on }: { id: LlmProviderId; on: boolean }) {
  const color = on ? 'var(--color-state-proposal)' : 'var(--t-faint)'
  return (
    <span className="flex items-center justify-center shrink-0" style={{
      width: 30, height: 30, borderRadius: 9,
      background: on ? 'color-mix(in srgb, var(--color-state-proposal) 12%, transparent)' : 'var(--t-box)',
      border: '0.5px solid var(--t-card-border)', color,
    }}>
      {id === 'local' ? (
        <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">
          <rect x="7" y="7" width="10" height="10" rx="2" /><path d="M10 3v3M14 3v3M10 18v3M14 18v3M3 10h3M3 14h3M18 10h3M18 14h3" />
        </svg>
      ) : (
        <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">
          <rect x="3" y="4" width="18" height="16" rx="2" /><path d="M7 9l3 3-3 3M13 15h4" />
        </svg>
      )}
    </span>
  )
}

function Badge({ text, tone }: { text: string; tone: 'accent' | 'warn' | 'muted' }) {
  const c = tone === 'accent' ? 'var(--color-state-proposal)'
    : tone === 'warn' ? 'var(--color-state-pending)' : 'var(--t-faint)'
  return (
    <span className="font-mono" style={{
      fontSize: 8.5, letterSpacing: '.1em', color: c,
      border: `0.5px solid ${tone === 'muted' ? 'var(--t-card-border)' : c}`,
      borderRadius: 4, padding: '1px 5px', whiteSpace: 'nowrap',
    }}>{text}</span>
  )
}

/** One card in the grid. */
function ProviderCard({ p, picked, missing, onPick }: {
  p: LlmProviderMeta; picked: boolean; missing: boolean; onPick: () => void
}) {
  return (
    <button onClick={onPick} className="flex flex-col text-left h-full"
      style={{
        gap: 8, padding: '13px 14px', borderRadius: 13, cursor: 'pointer',
        background: picked ? 'color-mix(in srgb, var(--color-state-proposal) 7%, transparent)' : 'var(--t-box)',
        border: `1px solid ${picked ? 'var(--color-state-proposal)' : 'var(--t-card-border)'}`,
        boxShadow: picked ? 'inset 0 0 0 1px color-mix(in srgb, var(--color-state-proposal) 22%, transparent)' : 'none',
        transition: 'background .14s, border-color .14s',
      }}>
      <div className="flex items-start" style={{ gap: 10 }}>
        <ProviderGlyph id={p.id} on={picked} />
        <div style={{ flex: 1, minWidth: 0 }}>
          <div className="flex items-center" style={{ gap: 8 }}>
            <span style={{ fontSize: 13.5, fontWeight: 500, color: 'var(--t-title)' }}>{p.name}</span>
          </div>
          <div className="flex items-center flex-wrap" style={{ gap: 5, marginTop: 4 }}>
            {p.recommended && <Badge text="RECOMMENDED" tone="accent" />}
            {p.kind === 'local' && <Badge text="PRIVATE" tone="muted" />}
            {missing && <Badge text="NOT INSTALLED" tone="warn" />}
          </div>
        </div>
        {/* Radio dot */}
        <span className="flex items-center justify-center shrink-0" style={{
          width: 17, height: 17, borderRadius: 99, marginTop: 1,
          border: `1.5px solid ${picked ? 'var(--color-state-proposal)' : 'var(--t-card-border)'}`,
          background: picked ? 'var(--color-state-proposal)' : 'transparent',
        }}>
          {picked && <span style={{ width: 6, height: 6, borderRadius: 99, background: '#fff' }} />}
        </span>
      </div>

      <p style={{ fontSize: 11.5, lineHeight: 1.4, color: 'var(--t-faint)' }}>{p.blurb}</p>

      {/* Picking an uninstalled CLI is allowed, not blocked — show exactly how to get it. */}
      {missing && picked && (
        <p className="font-mono" style={{ fontSize: 10.5, lineHeight: 1.5, color: 'var(--t-faint-2)', background: 'var(--t-card)', borderRadius: 6, padding: '6px 8px' }}>
          {p.installHint}
        </p>
      )}
    </button>
  )
}

export interface LlmProviderPickerProps {
  value: LlmProviderId
  onChange: (id: LlmProviderId) => void
  status: Record<string, ProviderStatus>
  scanning: boolean
  rescan: () => void
}

export default function LlmProviderPicker({ value, onChange, status, scanning, rescan }: LlmProviderPickerProps) {
  const picked = LLM_PROVIDERS.find((p) => p.id === value)
  const usingCli = picked?.kind === 'cli'

  return (
    <div className="flex flex-col" style={{ gap: 12 }}>
      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(2, minmax(0, 1fr))', gap: 9 }}>
        {LLM_PROVIDERS.map((p) => {
          const isPicked = p.id === value
          // Unknown (probe failed) is NOT "missing" — only say "not installed" when we
          // actually looked and didn't find it.
          const probed = status[p.id]
          const missing = p.kind === 'cli' && !!probed && !probed.installed
          return (
            <ProviderCard key={p.id} p={p} picked={isPicked} missing={missing} onPick={() => onChange(p.id)} />
          )
        })}
      </div>

      {/* Usage-footprint reassurance — only meaningful when a paid CLI is in play. */}
      {usingCli && (
        <p style={{ fontSize: 11, lineHeight: 1.45, color: 'var(--t-faint)' }}>
          {USAGE_FOOTPRINT_NOTE}
        </p>
      )}

      {/* Honesty on privacy: we disable telemetry on every call, but opting your prompts
          out of model TRAINING is a one-time account setting we can't flip for you. */}
      {usingCli && picked?.privacyUrl && (
        <div className="flex items-start" style={{
          gap: 8, padding: '10px 12px', borderRadius: 10,
          background: 'var(--t-box)', border: '0.5px solid var(--t-card-border)',
        }}>
          <span className="shrink-0" style={{ marginTop: 1, color: 'var(--color-state-approved)' }} aria-hidden="true">🔒</span>
          <p style={{ fontSize: 11, lineHeight: 1.5, color: 'var(--t-muted)' }}>
            Meridian turns off usage telemetry on every call. To also keep your prompts out of model
            training, switch on {picked.name}’s privacy setting once:{' '}
            <button onClick={() => picked.privacyUrl && openExternal(picked.privacyUrl)}
              style={{ color: 'var(--color-state-proposal)', background: 'none', border: 'none', padding: 0, cursor: 'pointer', font: 'inherit', textDecoration: 'underline' }}>
              {picked.privacyLabel ?? 'open settings'} ↗
            </button>
          </p>
        </div>
      )}

      <div className="flex items-center justify-between">
        <p style={{ fontSize: 11, color: 'var(--t-faint)', lineHeight: 1.45, flex: 1, paddingRight: 12 }}>
          Meridian uses the login you already have in that CLI - it never asks for an API key. If it
          turns out not to be signed in, that hour quietly falls back to on-device.
        </p>
        <button onClick={rescan} disabled={scanning} className="font-mono shrink-0"
          style={{
            fontSize: 10, letterSpacing: '.08em', textTransform: 'uppercase',
            color: 'var(--t-faint)', border: '0.5px solid var(--t-card-border)',
            borderRadius: 6, padding: '4px 9px', cursor: scanning ? 'default' : 'pointer',
            opacity: scanning ? 0.55 : 1, background: 'transparent',
          }}>
          {scanning ? 'Scanning…' : 'Rescan'}
        </button>
      </div>
    </div>
  )
}
