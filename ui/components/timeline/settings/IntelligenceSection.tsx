//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Settings → Intelligence. The post-setup home of the same AI-provider choice the
// wizard makes on its Intelligence step — same list, same <LlmProviderPicker>, so the
// two surfaces can never drift. Switching here takes effect on the NEXT hour with nothing
// to restart: the resolver re-reads settings.json on every call (src/llm/resolver.rs), so
// there is deliberately no reload_daemon() here.

'use client'

import { useCallback, useState } from 'react'
import type { RuntimeSettings } from '@/lib/settings'
import { llmProvider, type LlmProviderId } from '@/lib/llm-providers'
import LlmProviderPicker, { useLlmProviderDetection } from '@/components/LlmProviderPicker'
import type { SaveStatus } from './fields'

export function IntelligenceSection({ settings, patch, save }: {
  settings: RuntimeSettings
  patch: (changes: Partial<RuntimeSettings>) => void
  save: (fields: Partial<RuntimeSettings>, setStatus?: (s: SaveStatus) => void) => Promise<void>
}) {
  const { status, scanning, testingIds, testOne, rescan } = useLlmProviderDetection()
  const [saveStatus, setSaveStatus] = useState<SaveStatus>('idle')

  const provider = settings.llm_provider
  const picked = llmProvider(provider)
  const localReady = settings.llm_local_chat_model_ready
  const missing = picked.kind === 'cli' && status[picked.id]?.installed === false

  // The chosen provider's last real connectivity test, if any — not a guess. When it
  // failed, the "no fallback" warning below can say what's ACTUALLY happening right now
  // instead of only what could theoretically happen.
  const selectedTest = picked.kind === 'cli' ? status[picked.id]?.last_test ?? null : null
  const selectedBroken = !!selectedTest && selectedTest.outcome.status !== 'ok'

  const onChange = useCallback((id: LlmProviderId) => {
    if (id === provider) return
    // Optimistic: reflect the pick immediately, then persist. `save` rolls the store back
    // to the server's answer on failure (it setSettings to the response), so a rejected
    // write can't leave the UI claiming a provider the daemon isn't running.
    patch({ llm_provider: id })
    save({ llm_provider: id }, setSaveStatus)
  }, [provider, patch, save])

  return (
    <div className="max-w-[640px] flex flex-col gap-5">
      <div>
        <p className="mt-label" style={{ color: 'var(--color-state-proposal)' }}>Your AI</p>
        <h1 className="mt-title-lg mt-1.5" style={{ color: 'var(--t-title)' }}>Intelligence</h1>
        <p className="mt-body-sm mt-2 max-w-[520px]" style={{ color: 'var(--t-muted)' }}>
          The AI that writes your hourly summaries. Use a coding-agent CLI you already pay for, or
          keep it entirely on-device. Takes effect from the next hour - nothing to restart.
        </p>
      </div>

      <LlmProviderPicker
        value={provider}
        onChange={onChange}
        status={status}
        scanning={scanning}
        testingIds={testingIds}
        testOne={testOne}
        rescan={rescan}
      />

      {/* Save feedback — mirrors the other sections' inline status. */}
      {saveStatus === 'error' && (
        <p className="mt-body-sm" style={{ color: 'var(--status-error-dot)' }}>
          Couldn&apos;t save that choice. Try again.
        </p>
      )}

      {/* A KNOWN failure, from the last real test — stronger than the hypothetical warning
          below, since it says what IS happening, not what could. */}
      {selectedBroken && selectedTest && selectedTest.outcome.status !== 'ok' && (
        <div className="rounded-xl p-3.5 flex items-start gap-2.5"
          style={{ border: '1px solid var(--status-error-dot)', background: 'color-mix(in srgb, var(--status-error-dot) 7%, transparent)' }}>
          <span className="shrink-0" style={{ marginTop: 2, color: 'var(--status-error-dot)' }} aria-hidden="true">⚠</span>
          <p className="mt-body-sm" style={{ color: 'var(--t-muted)' }}>
            {selectedTest.outcome.status === 'rate_limited'
              ? `${picked.name} is currently rate-limited: ${selectedTest.outcome.message}. `
              : `${picked.name} isn't responding right now: ${selectedTest.outcome.message}. `}
            {localReady
              ? 'Hours are falling back to the on-device model until this clears.'
              : "There's no on-device fallback downloaded, so hours are being skipped until this clears."}
          </p>
        </div>
      )}

      {/* A CLI provider always keeps the on-device model as its safety net (a failed or
          rate-limited call falls back to it), so warn when that net isn't actually
          downloaded. This should be rare - the model is fetched during setup and is also
          needed for classification whatever provider is chosen. */}
      {picked.kind === 'cli' && !localReady && (
        <div className="rounded-xl p-3.5 flex items-start gap-2.5"
          style={{ border: '1px solid var(--color-state-pending)', background: 'color-mix(in srgb, var(--color-state-pending) 7%, transparent)' }}>
          <span className="shrink-0" style={{ marginTop: 2, color: 'var(--color-state-pending)' }} aria-hidden="true">△</span>
          <p className="mt-body-sm" style={{ color: 'var(--t-muted)' }}>
            The on-device model isn&apos;t downloaded yet, so if {picked.name} is unavailable there&apos;s
            nothing to fall back to and that hour will be skipped. It downloads automatically the next
            time the model set is fetched.
          </p>
        </div>
      )}
    </div>
  )
}
