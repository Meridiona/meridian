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

import { useCallback, useEffect, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { load } from '@/lib/bridge'
import type { RuntimeSettings } from '@/lib/settings'
import { LLM_INTRO_TITLE, providerChoiceFields, type LlmProviderId } from '@/lib/llm-providers'
import LlmProviderPicker, { useLlmProviderDetection } from '@/components/LlmProviderPicker'
import { phaseFor } from '@/components/LlmProviderDetail'
import type { SaveStatus } from './fields'

export function IntelligenceSection({ settings, save, gate = false, onConnected }: {
  settings: RuntimeSettings
  save: (fields: Partial<RuntimeSettings>, setStatus?: (s: SaveStatus) => void) => Promise<void>
  /** Open on the subscription question rather than the provider grid. True only when this
   *  panel was opened BY the "Meridian needs an AI engine" path (the modal is locked), where
   *  the user has no working provider and is being set up rather than adjusting one. */
  gate?: boolean
  /** Fired once a provider choice has actually been WRITTEN and accepted.
   *
   *  This is what releases the required lock on the modal above, so it must never fire on
   *  anything weaker than a successful save - a callback on "the user looked at the picker"
   *  would unlock a step that is not done. It hangs off `commit`'s resolve, which only runs
   *  on a 'saved' status. */
  onConnected?: () => void
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
          if (s === 'saved') { onConnected?.(); resolve() }
          else if (s === 'error') reject(new Error("Couldn't save - try again."))
        })
      }),
    [save, onConnected],
  )

  // Which fields a provider pick writes is shared with the setup wizard (see
  // providerChoiceFields) - the two used to hand-roll it and silently drifted.
  const onChange = useCallback(
    (id: LlmProviderId, customId?: string) => commit(providerChoiceFields(id, customId)),
    [commit],
  )

  // ALREADY CONNECTED COUNTS AS CONNECTED.
  //
  // `commit` was the only thing that could satisfy the lock, which quietly assumed the user
  // always arrives here with nothing working and leaves having written a choice. The other
  // route is just as common and had no exit at all: the provider in settings.json is the one
  // they want, and it starts answering while they are on its screen - the detail view's
  // auto-test passes, or they sign back in and the silent re-test lands. `active` flips, the
  // panel replaces the "Use <provider>" button with "<provider> is writing your summaries",
  // and now there is no control left to press. The screen says everything is fine and offers
  // nothing to do, while the walkthrough waits on a callback that can no longer fire.
  //
  // So the lock hangs off PROOF rather than off a write: `phaseFor(...).kind === 'ok'` for the
  // provider settings.json already names - the identical test `LlmProviderDetail` uses for
  // `active`, imported rather than restated so the button and this cannot disagree about what
  // "connected" means.
  //
  // Gated on `gate` because firing it is not free: `onConnected` releases the modal and hands
  // the user back to whatever pulled them here. Under the gate that is the point. Opening
  // Settings from the toolbar to look at a provider that has been working for weeks would
  // otherwise throw the user into the planner for no reason they asked for.
  const handedBack = useRef<string | null>(null)
  useEffect(() => {
    if (!gate || !onConnected) return
    if (handedBack.current === provider) return
    const probed = status[provider]
    const phase = phaseFor(
      installingIds.has(provider),
      signingIds.has(provider),
      testingIds.has(provider),
      probed,
      probed?.last_test ?? null,
    )
    if (phase.kind !== 'ok') return
    handedBack.current = provider
    onConnected()
  }, [gate, onConnected, provider, status, installingIds, signingIds, testingIds])

  return (
    <div className="max-w-[760px] flex flex-col gap-5">
      {/* Suppressed under the gate. "Choose your AI provider" above "Do you have a
          subscription?" is two headings asking different questions, and the second one is
          the one the user must answer first - the gate and the Groq walkthrough each carry
          their own title. The Dev disconnect stays either way; it is how this state is
          reached at all while the flow is being worked on. */}
      {gate ? (
        <div className="flex justify-end"><DevDisconnect provider={provider} onDone={rescan} /></div>
      ) : (
        <div>
          <div className="flex items-start justify-between gap-4">
            <p className="mt-label" style={{ color: 'var(--t-accent)' }}>Your AI</p>
            <DevDisconnect provider={provider} onDone={rescan} />
          </div>
          {/* Title only. The sub-heading under it explained what an AI provider is for and
              then told the user to pick one - both of which the four tiles below now say
              better: each names what it runs on, and each carries the same rank badges the
              gate uses. A paragraph that restates the screen delays it. (The setup wizard
              still shows it as its step subtitle, where nothing else has introduced the
              step yet.) */}
          <h1 className="mt-title-lg mt-1.5" style={{ color: 'var(--t-title)' }}>{LLM_INTRO_TITLE}</h1>
        </div>
      )}

      <LlmProviderPicker
        gate={gate}
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

/** Dev-only "Disconnect" — puts the app back into the not-connected state that
 *  drives the whole first-run AI flow.
 *
 *  Nothing in the shipped UI can un-connect a provider: the picker records
 *  successes and never failures, so once a provider tests OK the only route back
 *  is editing `provider_test_cache.json` by hand. That state is what the
 *  "Meridian needs an AI engine" prompt, the locked provider screen and the
 *  walkthrough's provider beat all hang off, so it needs to be one click to reach
 *  while any of them is being worked on.
 *
 *  Visibility rides `get_app_info().channel === 'dev'` — the same
 *  `cfg!(debug_assertions)` signal the LLM Lab uses — and the command refuses in
 *  release builds regardless of what the UI does. */
function DevDisconnect({ provider, onDone }: { provider: string; onDone: () => void }) {
  const [channel, setChannel] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  useEffect(() => {
    load<{ channel?: string }>('/api/version', 'get_app_info')
      .then(i => setChannel(i?.channel ?? null))
      .catch(() => setChannel(null))
  }, [])
  if (channel !== 'dev' || !provider) return null
  return (
    <button
      onClick={async () => {
        setBusy(true)
        try {
          await invoke('disconnect_llm_provider', { id: provider })
          // Re-detect so the picker's own cards show the disconnected state too,
          // rather than this silently disagreeing with what is on screen.
          onDone()
        } catch { /* dev affordance - a failure here is not worth a banner */ }
        setBusy(false)
      }}
      disabled={busy}
      title="Dev only - marks this provider signed out so the first-run AI flow can be re-tested"
      className="mt-body-sm px-2.5 py-1 rounded-md shrink-0"
      style={{
        color: 'var(--t-muted)', fontWeight: 700,
        border: '1px dashed var(--t-ctrl-border)',
        opacity: busy ? 0.6 : 1,
      }}>
      {busy ? 'Disconnecting…' : 'Disconnect (dev)'}
    </button>
  )
}
