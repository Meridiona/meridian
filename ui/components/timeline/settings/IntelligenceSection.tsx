//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Settings → Intelligence. The post-setup home of the same AI-provider choice the wizard makes
// on its Intelligence step — same list, same <LlmProviderPicker>, so the two surfaces can never
// drift. Switching here takes effect on the NEXT hour with nothing to restart: the resolver
// re-reads settings.json on every call (src/llm/resolver.rs), so there is no reload_daemon().
//
// A pick COMMITS IMMEDIATELY (there is no staged Save anymore): the user opens a provider's
// detail view and the "Use <provider>" button is itself the explicit, deliberate write — so the
// old optimistic-patch-survives-a-failed-write problem can't happen. `save` only updates the
// in-memory settings on success and reports failure through the status callback, which the
// detail view surfaces; on failure `settings.llm_provider` is unchanged, so nothing rolls back.
// There is no model control: the model always follows the provider's own default, and picking
// a built-in provider CLEARS any override an older build left behind.

'use client'

import { useCallback } from 'react'
import type { RuntimeSettings } from '@/lib/settings'
import { LLM_INTRO_BODY, LLM_INTRO_TITLE, providerChoiceFields, type LlmProviderId } from '@/lib/llm-providers'
import LlmProviderPicker, { useLlmProviderDetection } from '@/components/LlmProviderPicker'
import type { SaveStatus } from './fields'

export function IntelligenceSection({ settings, save }: {
  settings: RuntimeSettings
  save: (fields: Partial<RuntimeSettings>, setStatus?: (s: SaveStatus) => void) => Promise<void>
}) {
  const { status, scanning, testingIds, installingIds, signingIds, testOne, install, signIn, rescan } = useLlmProviderDetection()

  const provider = settings.llm_provider
  const selectedCustomId = settings.llm_provider_custom_id ?? null

  // Commit a change and resolve/reject on the real save outcome, so the detail view can show
  // "Switching…" and a failure. `save` never rejects itself (it reports via the callback), so
  // the promise bridge is here.
  const commit = useCallback(
    (fields: Partial<RuntimeSettings>) =>
      new Promise<void>((resolve, reject) => {
        save(fields, (s) => {
          if (s === 'saved') resolve()
          else if (s === 'error') reject(new Error("Couldn't save - try again."))
        })
      }),
    [save],
  )

  // Which fields a provider pick writes is shared with the setup wizard (see
  // providerChoiceFields) - the two used to hand-roll it and silently drifted.
  const onChange = useCallback(
    (id: LlmProviderId, customId?: string) => commit(providerChoiceFields(id, customId)),
    [commit],
  )

  return (
    <div className="max-w-[760px] flex flex-col gap-5">
      <div>
        <p className="mt-label" style={{ color: 'var(--color-state-proposal)' }}>Your AI</p>
        <h1 className="mt-title-lg mt-1.5" style={{ color: 'var(--t-title)' }}>{LLM_INTRO_TITLE}</h1>
        <p className="mt-body-sm mt-2 max-w-[560px]" style={{ color: 'var(--t-muted)' }}>{LLM_INTRO_BODY}</p>
      </div>

      <LlmProviderPicker
        value={provider}
        selectedCustomId={selectedCustomId}
        onChange={onChange}
        status={status}
        scanning={scanning}
        testingIds={testingIds}
        installingIds={installingIds}
        signingIds={signingIds}
        testOne={testOne}
        install={install}
        signIn={signIn}
        rescan={rescan}
      />
    </div>
  )
}
