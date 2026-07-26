//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The AI-provider picker — ONE component, mounted in both the setup wizard's Intelligence step
// and the dashboard's Settings, so the two surfaces can never drift. The provider list lives in
// `@/lib/llm-providers`.
//
// Two levels, on purpose (this replaced a flat grid of cards each carrying every control at
// once, which read as busy and buried the one decision):
//
//   CHOOSER  — three recommended coding-agent tiles (logo + name + RECOMMENDED) plus one
//              "bring your own API key" tile. Just the choice, nothing else.
//   DETAIL   — click a tile and you get its own screen: install the CLI if missing, confirm the
//              sign-in if it's there, and make it the default. One
//              provider, one next action.
//
// It deliberately does NOT save. The wizard writes each pick straight through; Settings commits
// through its own save(). The owner does the write and this stays a controlled input.

import { useCallback, useEffect, useState } from 'react'
import {
  CHOOSER_PROVIDER_IDS, LLM_RECOMMENDED_BADGE, LLM_RECOMMENDED_NOTE,
  llmProvider, type LlmProviderId,
} from '@/lib/llm-providers'
import { invoke } from '@/lib/bridge'
import type {
  InstallOutcome, ProviderStatus, ProviderTestOutcome, ProviderTestResult,
} from '@/lib/api-types'
import { ProviderLogo } from '@/components/LlmProviderLogos'
import LlmProviderDetail from '@/components/LlmProviderDetail'

// Re-exported so the existing `from '@/components/LlmProviderPicker'` imports
// keep resolving. The definitions now live in api-types.ts, per the repo rule
// that Rust-mirrored invoke contracts live in one place.
export type {
  InstallOutcome, ProviderStatus, ProviderTestOutcome, ProviderTestResult,
}
import { AddCustomProvider, CustomProviderCard, useCustomProviders } from '@/components/CustomProviders'


/**
 * Probe which provider CLIs exist on this Mac (free, instant). Unlike before, this NO LONGER
 * fires a real connectivity test for every installed CLI on mount — that spent a request per
 * provider and surfaced scary failures for CLIs the user never even opened (e.g. a stray
 * cursor-agent that isn't signed in). Testing now happens in the DETAIL view, only for the
 * provider the user actually opened. `install` runs the vendor installer, then re-detects and
 * auto-tests just that one.
 */
export function useLlmProviderDetection() {
  const [status, setStatus] = useState<Record<string, ProviderStatus>>({})
  const [scanning, setScanning] = useState(true)
  const [testingIds, setTestingIds] = useState<Set<string>>(new Set())
  const [installingIds, setInstallingIds] = useState<Set<string>>(new Set())
  const [signingIds, setSigningIds] = useState<Set<string>>(new Set())

  /** Free install-state probe. Re-run on demand (Rescan / after a manual install). */
  const detect = useCallback(async () => {
    setScanning(true)
    try {
      const found = await invoke<ProviderStatus[]>('detect_llm_providers')
      setStatus(Object.fromEntries(found.map((p) => [p.id, p])))
    } catch {
      // A failed probe must not block the step: an un-probed provider renders as "can't tell",
      // and picking it is still allowed.
      setStatus({})
    } finally {
      setScanning(false)
    }
  }, [])

  /** One real connectivity test — spends one request against the user's own subscription.
   *  `silent` runs + persists the test WITHOUT the "Testing…" spinner, so a confirm-in-the
   *  background (e.g. right after sign-in) doesn't yank the panel back to a spinner. */
  const testOne = useCallback(async (id: string, opts?: { silent?: boolean }) => {
    if (!opts?.silent) setTestingIds((prev) => new Set(prev).add(id))
    try {
      const result = await invoke<ProviderTestResult>('test_llm_provider', { id })
      setStatus((prev) => (prev[id] ? { ...prev, [id]: { ...prev[id], last_test: result } } : prev))
    } catch {
      // A failed probe CALL isn't evidence the provider stopped working — keep what was cached.
    } finally {
      if (!opts?.silent) {
        setTestingIds((prev) => {
          const next = new Set(prev)
          next.delete(id)
          return next
        })
      }
    }
  }, [])

  /** Run the vendor installer for one provider, then re-detect and (on success) auto-test it. */
  const install = useCallback(async (id: string): Promise<InstallOutcome> => {
    setInstallingIds((prev) => new Set(prev).add(id))
    try {
      const outcome = await invoke<InstallOutcome>('install_llm_provider', { id })
      await detect()
      if (outcome.ok) await testOne(id)
      return outcome
    } catch (e) {
      return { ok: false, message: String(e), path: null, command: '' }
    } finally {
      setInstallingIds((prev) => {
        const next = new Set(prev)
        next.delete(id)
        return next
      })
    }
  }, [detect, testOne])

  /** Run an interactive browser sign-in for a provider whose CLI authenticates against the
   *  user's own subscription - Cursor (`cursor_sign_in`), Codex (`codex_sign_in`), or Claude
   *  (`claude_sign_in`). Each tray command drives that vendor's `… login` and opens the browser;
   *  none takes a provider argument, so the id picks the command here. */
  const signIn = useCallback(async (id: string): Promise<InstallOutcome> => {
    setSigningIds((prev) => new Set(prev).add(id))
    try {
      const command =
        id === 'codex' ? 'codex_sign_in' : id === 'claude' ? 'claude_sign_in' : 'cursor_sign_in'
      const outcome = await invoke<InstallOutcome>(command)
      if (outcome.ok) {
        // `cursor-agent login` exits 0 only AFTER the browser OAuth completes, so this is the
        // exact moment the user finished signing in. Flip the panel to "connected" NOW rather
        // than making them wait on a slow follow-up completion call, then confirm + persist it
        // with a silent background test (which corrects the badge if it somehow isn't usable).
        const result: ProviderTestResult = {
          id, outcome: { status: 'ok' }, elapsed_ms: 0, tested_at: new Date().toISOString(),
        }
        setStatus((prev) => {
          const cur = prev[id] ?? { id, installed: true, path: null, authenticated: null, last_test: null }
          return { ...prev, [id]: { ...cur, installed: true, last_test: result } }
        })
        void testOne(id, { silent: true })
      } else {
        // Sign-in didn't complete - refresh at least the install state.
        await detect()
      }
      return outcome
    } catch (e) {
      return { ok: false, message: String(e), path: null, command: '' }
    } finally {
      setSigningIds((prev) => {
        const next = new Set(prev)
        next.delete(id)
        return next
      })
    }
  }, [detect, testOne])

  useEffect(() => { detect() }, [detect])
  return { status, scanning, testingIds, installingIds, signingIds, testOne, install, signIn, rescan: detect }
}

/** One tile in the top-level chooser: logo + name + a badge, nothing else.
 *  `warning`, when set, replaces the normal badge/subtitle with a confirmed problem —
 *  either "not installed" (a real probe came up empty) or "erroring" (the last real
 *  test failed, e.g. a spawn error). Both come from real signals, never an
 *  unprobed/slow state, so a card never falsely flags a provider that's actually fine. */
function ChooserTile({ id, name, badge, selected, subtitle, warning, onOpen }: {
  id: LlmProviderId; name: string; badge: string; selected: boolean
  subtitle?: string; warning?: { label: string; message: string }; onOpen: () => void
}) {
  return (
    <button onClick={onOpen} className="flex flex-col text-left h-full"
      style={{
        gap: 9, padding: '15px 15px', borderRadius: 13, cursor: 'pointer',
        background: selected ? 'color-mix(in srgb, var(--color-state-proposal) 10%, var(--t-card))' : 'var(--t-card)',
        border: `1px solid ${selected ? 'var(--color-state-proposal)' : 'var(--t-ctrl-border)'}`,
        boxShadow: '0 1px 2px rgba(0,0,0,.04)',
      }}>
      <div className="flex items-start justify-between" style={{ gap: 10 }}>
        <span className="flex items-center justify-center shrink-0" style={{
          width: 38, height: 38, borderRadius: 10,
          background: selected ? 'color-mix(in srgb, var(--color-state-proposal) 15%, transparent)' : 'var(--t-box)',
          border: '1px solid var(--t-ctrl-border)',
          color: selected ? 'var(--color-state-proposal)' : 'var(--t-title)',
        }}>
          <ProviderLogo id={id} size={21} />
        </span>
        {selected && (
          // Selected-but-confirmed-broken must not say "IN USE" — that reads as
          // "working", which is exactly wrong when the daemon can't spawn/find it.
          warning ? (
            <span className="font-mono" style={{
              fontSize: 9, letterSpacing: '.09em', color: 'var(--color-state-pending)',
              border: '1px solid var(--color-state-pending)',
              background: 'color-mix(in srgb, var(--color-state-pending) 10%, transparent)',
              borderRadius: 4, padding: '1.5px 5px', whiteSpace: 'nowrap',
            }}>{warning.label}</span>
          ) : (
            <span className="font-mono" style={{
              fontSize: 9, letterSpacing: '.09em', color: 'var(--color-state-approved)',
              border: '1px solid var(--color-state-approved)',
              background: 'color-mix(in srgb, var(--color-state-approved) 10%, transparent)',
              borderRadius: 4, padding: '1.5px 5px', whiteSpace: 'nowrap',
            }}>IN USE</span>
          )
        )}
      </div>
      <div className="flex flex-col" style={{ gap: 4 }}>
        <span style={{ fontSize: 14.5, fontWeight: 550, color: 'var(--t-title)' }}>{name}</span>
        <span className="font-mono self-start" style={{
          fontSize: 8.5, letterSpacing: '.09em', color: 'var(--t-faint)',
        }}>{badge}</span>
        {warning ? (
          <span style={{ fontSize: 11, lineHeight: 1.4, color: 'var(--color-state-pending)' }}>
            {warning.message}
          </span>
        ) : subtitle && <span style={{ fontSize: 11, lineHeight: 1.4, color: 'var(--t-muted)' }}>{subtitle}</span>}
      </div>
    </button>
  )
}

export interface LlmProviderPickerProps {
  value: LlmProviderId
  /** Which custom endpoint, when `value` is 'custom'. */
  selectedCustomId?: string | null
  /** The owner persists BOTH fields together (`llm_provider` + `llm_provider_custom_id`). May
   *  return a promise the detail view awaits to show "Switching…"/failure. */
  onChange: (id: LlmProviderId, customId?: string) => void | Promise<void>
  status: Record<string, ProviderStatus>
  scanning: boolean
  testingIds: Set<string>
  installingIds: Set<string>
  signingIds: Set<string>
  testOne: (id: string) => void
  install: (id: string) => Promise<InstallOutcome>
  signIn: (id: string) => Promise<InstallOutcome>
  rescan: () => void
}

export default function LlmProviderPicker({
  value, selectedCustomId, onChange, status, scanning, testingIds, installingIds, signingIds,
  testOne, install, signIn, rescan,
}: LlmProviderPickerProps) {
  // Which provider's detail is open, or null for the chooser. `'custom'` opens the BYO flow.
  const [openId, setOpenId] = useState<LlmProviderId | null>(null)
  const custom = useCustomProviders()

  // A provider the user has selected but that isn't one of the three recommended tiles (a
  // legacy Copilot pick). We don't render a Copilot card, but we mustn't strand them either.
  const usingHidden = value !== 'custom' && !CHOOSER_PROVIDER_IDS.includes(value)

  if (openId && openId !== 'custom') {
    const p = llmProvider(openId)
    return (
      <LlmProviderDetail
        p={p}
        probed={status[openId]}
        testing={testingIds.has(openId)}
        installing={installingIds.has(openId)}
        signing={signingIds.has(openId)}
        selected={value === openId}
        onBack={() => setOpenId(null)}
        onInstall={() => install(openId)}
        onSignIn={() => signIn(openId)}
        onTest={() => testOne(openId)}
        onUse={async () => { await onChange(openId) }}
      />
    )
  }

  if (openId === 'custom') {
    return (
      <CustomDetail
        value={value}
        selectedCustomId={selectedCustomId ?? null}
        custom={custom}
        onBack={() => setOpenId(null)}
        onPick={(id) => onChange('custom', id)}
      />
    )
  }

  return (
    <div className="flex flex-col" style={{ gap: 12 }}>
      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(190px, 1fr))', gap: 10 }}>
        {CHOOSER_PROVIDER_IDS.map((id) => {
          const p = llmProvider(id)
          const probed = status[id]
          const isSelected = value === id
          const warning =
            probed && !probed.installed
              ? {
                  label: 'NOT DETECTED',
                  message: isSelected
                    ? "Selected, but the CLI isn't installed on this machine."
                    : 'Not installed on this machine.',
                }
              : probed?.last_test?.outcome.status === 'failed'
                ? { label: 'ERROR', message: probed.last_test.outcome.message }
                : undefined
          return (
            <ChooserTile
              key={id}
              id={id}
              name={p.name}
              badge={LLM_RECOMMENDED_BADGE}
              selected={isSelected}
              warning={warning}
              onOpen={() => setOpenId(id)}
            />
          )
        })}
        <ChooserTile
          id="custom"
          name="Bring your own API key"
          badge="ADVANCED"
          selected={value === 'custom'}
          subtitle="Your own OpenAI-compatible endpoint."
          onOpen={() => setOpenId('custom')}
        />
      </div>

      <p style={{ fontSize: 11, lineHeight: 1.45, color: 'var(--t-muted)' }}>
        {LLM_RECOMMENDED_NOTE}
      </p>

      {/* A legacy Copilot user isn't stranded: tell them what they're on and let them switch. */}
      {usingHidden && (
        <p style={{ fontSize: 11, lineHeight: 1.45, color: 'var(--color-state-pending)' }}>
          You&apos;re currently using {llmProvider(value).name}. Pick one of the recommended providers
          above to switch.
        </p>
      )}

      {/* Re-probe for a CLI the user installed in a terminal while this was open. */}
      <button onClick={rescan} disabled={scanning} className="self-start"
        style={{
          fontSize: 11, color: 'var(--t-muted)', background: 'none', border: 'none',
          padding: 0, cursor: scanning ? 'default' : 'pointer', textDecoration: 'underline',
        }}>
        {scanning ? 'Checking what’s installed…' : 'Installed a CLI yourself? Rescan'}
      </button>
    </div>
  )
}

/** The bring-your-own-key detail — the existing custom-endpoint registry, behind a Back button. */
function CustomDetail({ value, selectedCustomId, custom, onBack, onPick }: {
  value: LlmProviderId
  selectedCustomId: string | null
  custom: ReturnType<typeof useCustomProviders>
  onBack: () => void
  onPick: (id: string) => void
}) {
  return (
    <div className="flex flex-col" style={{ gap: 13 }}>
      <button onClick={onBack} className="self-start flex items-center"
        style={{ gap: 6, fontSize: 12, color: 'var(--t-muted)', background: 'none', border: 'none', cursor: 'pointer', padding: 0 }}>
        <svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round"><path d="M10 4 6 8l4 4" /></svg>
        All providers
      </button>
      <div>
        <span style={{ fontSize: 17, fontWeight: 600, color: 'var(--t-title)' }}>Bring your own API key</span>
        <p style={{ fontSize: 12, lineHeight: 1.45, color: 'var(--t-muted)', marginTop: 3, maxWidth: 520 }}>
          Point Meridian at your own OpenAI-compatible endpoint. Unlike the coding agents, this
          bills your own key for every call - so a free-tier key is the right choice.
        </p>
      </div>

      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(232px, 1fr))', gap: 9 }}>
        {custom.providers.map((c) => (
          <CustomProviderCard
            key={c.id}
            p={c}
            picked={value === 'custom' && selectedCustomId === c.id}
            probing={custom.probingIds.has(c.id)}
            onPick={() => onPick(c.id)}
            onProbe={(hard) => custom.probe(c.id, hard)}
            onRemove={() => custom.remove(c.id)}
          />
        ))}
        {!custom.loading && <AddCustomProvider onAdd={custom.add} />}
      </div>
    </div>
  )
}
