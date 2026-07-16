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

/** What one real connectivity test found (mirrors `ProviderTestOutcome` in src/llm/detect.rs). */
export type ProviderTestOutcome =
  | { status: 'ok' }
  | { status: 'rate_limited'; message: string }
  | { status: 'failed'; message: string }

/** One recorded test run (mirrors `ProviderTestResult` in src/llm/detect.rs). */
export interface ProviderTestResult {
  id: string
  outcome: ProviderTestOutcome
  elapsed_ms: number
  /** RFC3339 — when this test ran. */
  tested_at: string
}

/** One provider's live install state (mirrors `ProviderStatus` in src/llm/detect.rs). */
export interface ProviderStatus {
  id: string
  installed: boolean
  path: string | null
  /** Always null — Meridian reports *installed*, not *signed in*. See src/llm/detect.rs. */
  authenticated: boolean | null
  /** The last real connectivity test on record, if any. `null` means never tested — not
   *  failed. A stale test (from before the CLI was reinstalled/re-authed) is still shown;
   *  Rescan and the per-card Test button are how the user refreshes it. */
  last_test: ProviderTestResult | null
}

/**
 * Probe which provider CLIs exist on this Mac (free, instant), then - for every one that
 * IS installed - run a real connectivity test (spends one request per provider against
 * the user's own subscription). Re-run on demand (the Rescan button): the user will
 * alt-tab out to `npm i -g …` or `claude login` and come back mid-wizard, and a cached
 * "not installed"/"not working" would then be a lie.
 */
export function useLlmProviderDetection() {
  const [status, setStatus] = useState<Record<string, ProviderStatus>>({})
  const [scanning, setScanning] = useState(true)
  const [testingIds, setTestingIds] = useState<Set<string>>(new Set())

  /** Re-test ONE provider on demand (a card's own Test button) without re-testing every
   *  other installed provider - e.g. right after fixing that CLI's login. */
  const testOne = useCallback(async (id: string) => {
    setTestingIds((prev) => new Set(prev).add(id))
    try {
      const result = await invoke<ProviderTestResult>('test_llm_provider', { id })
      setStatus((prev) => (prev[id] ? { ...prev, [id]: { ...prev[id], last_test: result } } : prev))
    } catch {
      // Leave whatever was cached before - a failed probe call itself is not evidence
      // the provider stopped working, just that we couldn't find out right now.
    } finally {
      setTestingIds((prev) => {
        const next = new Set(prev)
        next.delete(id)
        return next
      })
    }
  }, [])

  const rescan = useCallback(async () => {
    setScanning(true)
    let found: ProviderStatus[] = []
    try {
      found = await invoke<ProviderStatus[]>('detect_llm_providers')
      setStatus(Object.fromEntries(found.map((p) => [p.id, p])))
    } catch {
      // A failed probe must not block the step: an un-probed provider renders as
      // "can't tell", and picking it is still allowed (the CLI may well be there).
      setStatus({})
    } finally {
      setScanning(false)
    }

    // The expensive half: a real call per installed CLI, run concurrently server-side.
    // Never spends a request on a provider that isn't even on the machine.
    const installedIds = found.filter((p) => p.installed).map((p) => p.id)
    if (installedIds.length === 0) return
    setTestingIds(new Set(installedIds))
    try {
      const results = await invoke<ProviderTestResult[]>('test_all_llm_providers')
      setStatus((prev) => {
        const next = { ...prev }
        for (const r of results) {
          if (next[r.id]) next[r.id] = { ...next[r.id], last_test: r }
        }
        return next
      })
    } catch {
      // Same reasoning as above - stale-but-cached beats blocking the panel.
    } finally {
      setTestingIds(new Set())
    }
  }, [])

  useEffect(() => { rescan() }, [rescan])
  return { status, scanning, testingIds, testOne, rescan }
}

/** "3m ago" / "2h ago" / "5d ago" from an RFC3339 timestamp. Never throws on a bad string -
 *  falls back to blank, since a badge with no relative time is still honest. */
function timeAgo(iso: string): string {
  const then = Date.parse(iso)
  if (Number.isNaN(then)) return ''
  const secs = Math.max(0, Math.floor((Date.now() - then) / 1000))
  if (secs < 60) return 'just now'
  const mins = Math.floor(secs / 60)
  if (mins < 60) return `${mins}m ago`
  const hours = Math.floor(mins / 60)
  if (hours < 24) return `${hours}h ago`
  return `${Math.floor(hours / 24)}d ago`
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

/** Badge + detail for the last real connectivity test, or nothing if never tested. */
function TestBadge({ testing, lastTest }: { testing: boolean; lastTest: ProviderTestResult | null }) {
  if (testing) return <Badge text="TESTING…" tone="muted" />
  if (!lastTest) return null
  const when = timeAgo(lastTest.tested_at)
  if (lastTest.outcome.status === 'ok') {
    return <Badge text={when ? `VERIFIED · ${when}` : 'VERIFIED'} tone="accent" />
  }
  if (lastTest.outcome.status === 'rate_limited') {
    return <Badge text={when ? `RATE LIMITED · ${when}` : 'RATE LIMITED'} tone="warn" />
  }
  return <Badge text={when ? `CONNECTION FAILED · ${when}` : 'CONNECTION FAILED'} tone="warn" />
}

/** One card in the grid. A `div` shell (not `button`) so a real Test button can nest
 *  inside without invalid nested-interactive-element markup; keyboard/role parity with a
 *  native button is kept by hand (role, tabIndex, Enter/Space). */
function ProviderCard({ p, picked, missing, testing, lastTest, onPick, onTest }: {
  p: LlmProviderMeta; picked: boolean; missing: boolean
  testing: boolean; lastTest: ProviderTestResult | null
  onPick: () => void; onTest: () => void
}) {
  const testable = p.kind === 'cli' && !missing
  return (
    <div
      role="button"
      tabIndex={0}
      onClick={onPick}
      onKeyDown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); onPick() } }}
      className="flex flex-col text-left h-full"
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
            <TestBadge testing={testing} lastTest={lastTest} />
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

      {/* The failure/rate-limit message from the last test, when there is one. */}
      {lastTest && lastTest.outcome.status !== 'ok' && (
        <p style={{ fontSize: 10.5, lineHeight: 1.4, color: 'var(--t-faint-2)' }}>
          {lastTest.outcome.message}
        </p>
      )}

      {/* Picking an uninstalled CLI is allowed, not blocked — show exactly how to get it. */}
      {missing && picked && (
        <p className="font-mono" style={{ fontSize: 10.5, lineHeight: 1.5, color: 'var(--t-faint-2)', background: 'var(--t-card)', borderRadius: 6, padding: '6px 8px' }}>
          {p.installHint}
        </p>
      )}

      {testable && (
        <button
          onClick={(e) => { e.stopPropagation(); onTest() }}
          disabled={testing}
          className="font-mono self-start"
          style={{
            fontSize: 9.5, letterSpacing: '.08em', textTransform: 'uppercase',
            color: 'var(--t-faint)', border: '0.5px solid var(--t-card-border)',
            borderRadius: 5, padding: '3px 7px', cursor: testing ? 'default' : 'pointer',
            opacity: testing ? 0.55 : 1, background: 'transparent',
          }}>
          {testing ? 'Testing…' : 'Test connection'}
        </button>
      )}
    </div>
  )
}

export interface LlmProviderPickerProps {
  value: LlmProviderId
  onChange: (id: LlmProviderId) => void
  status: Record<string, ProviderStatus>
  scanning: boolean
  /** Providers a real connectivity test is currently in flight for — from Rescan (all
   *  installed providers at once) or a single card's own Test button. */
  testingIds: Set<string>
  /** Run a real connectivity test for one provider on demand. */
  testOne: (id: string) => void
  rescan: () => void
}

export default function LlmProviderPicker({ value, onChange, status, scanning, testingIds, testOne, rescan }: LlmProviderPickerProps) {
  const picked = LLM_PROVIDERS.find((p) => p.id === value)
  const usingCli = picked?.kind === 'cli'
  const busy = scanning || testingIds.size > 0

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
            <ProviderCard
              key={p.id}
              p={p}
              picked={isPicked}
              missing={missing}
              testing={testingIds.has(p.id)}
              lastTest={probed?.last_test ?? null}
              onPick={() => onChange(p.id)}
              onTest={() => testOne(p.id)}
            />
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
        <button onClick={rescan} disabled={busy} className="font-mono shrink-0"
          style={{
            fontSize: 10, letterSpacing: '.08em', textTransform: 'uppercase',
            color: 'var(--t-faint)', border: '0.5px solid var(--t-card-border)',
            borderRadius: 6, padding: '4px 9px', cursor: busy ? 'default' : 'pointer',
            opacity: busy ? 0.55 : 1, background: 'transparent',
          }}>
          {scanning ? 'Scanning…' : testingIds.size > 0 ? 'Testing…' : 'Rescan'}
        </button>
      </div>
    </div>
  )
}
