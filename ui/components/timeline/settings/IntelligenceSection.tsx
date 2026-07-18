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
  const missing = picked.kind === 'cli' && status[picked.id]?.installed === false

  // The chosen provider's last real connectivity test, if any — not a guess. When it
  // failed, the "no fallback" warning below can say what's ACTUALLY happening right now
  // instead of only what could theoretically happen.
  const selectedTest = picked.kind === 'cli' ? status[picked.id]?.last_test ?? null : null
  const selectedBroken = !!selectedTest && selectedTest.outcome.status !== 'ok'

  const onChange = useCallback((id: LlmProviderId, customId?: string) => {
    if (id === provider && !customId) return
    // 'custom' names a KIND, so it always travels with the endpoint's id - either alone is
    // not a valid choice, and update_settings rejects a custom selection that names no
    // configured endpoint.
    const fields: Partial<RuntimeSettings> =
      id === 'custom' ? { llm_provider: id, llm_provider_custom_id: customId ?? null } : { llm_provider: id }
    // Optimistic: reflect the pick immediately, then persist. `save` rolls the store back
    // to the server's answer on failure (it setSettings to the response), so a rejected
    // write can't leave the UI claiming a provider the daemon isn't running.
    patch(fields)
    save(fields, setSaveStatus)
  }, [provider, patch, save])

  return (
    <div className="max-w-[640px] flex flex-col gap-5">
      <div>
        <p className="mt-label" style={{ color: 'var(--color-state-proposal)' }}>Your AI</p>
        <h1 className="mt-title-lg mt-1.5" style={{ color: 'var(--t-title)' }}>Intelligence</h1>
        <p className="mt-body-sm mt-2 max-w-[520px]" style={{ color: 'var(--t-muted)' }}>
          The AI that writes your hourly summaries. Use a coding-agent CLI you already pay for, or
          a custom cloud endpoint on your own key. Takes effect from the next hour - nothing to restart.
        </p>
      </div>

      <LlmProviderPicker
        value={provider}
        selectedCustomId={settings.llm_provider_custom_id}
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

      {/* A KNOWN failure, from the last real test: there is no fallback, so a broken
          provider means those hours are skipped until it clears. */}
      {selectedBroken && selectedTest && selectedTest.outcome.status !== 'ok' && (
        <div className="rounded-xl p-3.5 flex items-start gap-2.5"
          style={{ border: '1px solid var(--status-error-dot)', background: 'color-mix(in srgb, var(--status-error-dot) 7%, transparent)' }}>
          <span className="shrink-0" style={{ marginTop: 2, color: 'var(--status-error-dot)' }} aria-hidden="true">⚠</span>
          <p className="mt-body-sm" style={{ color: 'var(--t-muted)' }}>
            {selectedTest.outcome.status === 'rate_limited'
              ? `${picked.name} is currently rate-limited: ${selectedTest.outcome.message}. `
              : `${picked.name} isn't responding right now: ${selectedTest.outcome.message}. `}
            Hours are left pending and retried automatically until this clears.
          </p>
        </div>
      )}
    </div>
  )
}
