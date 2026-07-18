//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Settings → Intelligence. The post-setup home of the same AI-provider choice the
// wizard makes on its Intelligence step — same list, same <LlmProviderPicker>, so the
// two surfaces can never drift. Switching here takes effect on the NEXT hour with nothing
// to restart: the resolver re-reads settings.json on every call (src/llm/resolver.rs), so
// there is deliberately no reload_daemon() here.
//
// Unlike the wizard (where each pick writes straight through, because the wizard's own
// Next/Back is the commit), a pick HERE is only STAGED — it lands in `pending` and is
// persisted by an explicit Save, matching every other Settings section (Advanced,
// Capture, Notifications all use <SaveButton>). Staging also removes the old optimistic
// `patch()`-on-click: `save` only calls setSettings on SUCCESS (useRuntimeSettings.ts),
// so an optimistic patch survived a FAILED write and left the panel claiming a provider
// the daemon had rejected. Nothing is written until Save now, so there is nothing to
// roll back. Only the selection is gated — Test connection, Rescan, and adding/removing
// custom endpoints stay immediate, since those are actions, not this setting.

'use client'

import { useCallback, useState } from 'react'
import type { RuntimeSettings } from '@/lib/settings'
import { llmProvider, type LlmProviderId } from '@/lib/llm-providers'
import LlmProviderPicker, { useLlmProviderDetection } from '@/components/LlmProviderPicker'
import { ModelSelect, ModelUnsupportedNote } from '@/components/ModelPicker'
import { SaveButton, type SaveStatus } from './fields'

/** A staged, not-yet-saved provider choice. `customId` is the chosen endpoint when
 *  `id` is 'custom', else null — the two always travel together (see `onChange`).
 *  `model` is the optional per-provider model override; '' means the provider's default. */
interface PendingChoice {
  id: LlmProviderId
  customId: string | null
  model: string
}

export function IntelligenceSection({ settings, save }: {
  settings: RuntimeSettings
  save: (fields: Partial<RuntimeSettings>, setStatus?: (s: SaveStatus) => void) => Promise<void>
}) {
  const { status, scanning, testingIds, testOne, rescan } = useLlmProviderDetection()
  const [saveStatus, setSaveStatus] = useState<SaveStatus>('idle')
  const [pending, setPending] = useState<PendingChoice | null>(null)

  const savedId = settings.llm_provider
  const savedCustomId = settings.llm_provider_custom_id ?? null
  const savedModel = settings.llm_provider_model ?? ''
  // What the panel SHOWS: the staged pick while one is held, else what's on disk. Every
  // dependent read below (badges, warnings) uses this, so the whole section describes the
  // provider you are about to commit rather than half-describing the old one.
  const provider = pending?.id ?? savedId
  const selectedCustomId = pending ? pending.customId : savedCustomId
  const model = pending ? pending.model : savedModel
  const picked = llmProvider(provider)
  // The override is only meaningful for a CLI backend that actually passes it through.
  // A custom endpoint carries its model on the endpoint ROW instead (openai_compat sends
  // `ep.model` and ignores cfg.model), and copilot has no model flag at all.
  const showModelPicker = picked.kind === 'cli' && picked.supportsModelOverride

  // The chosen provider's last real connectivity test, if any — not a guess. When it
  // failed, the warning below says what's ACTUALLY happening right now instead of only
  // what could theoretically happen.
  const selectedTest = picked.kind === 'cli' ? status[picked.id]?.last_test ?? null : null
  const selectedBroken = !!selectedTest && selectedTest.outcome.status !== 'ok'

  const onChange = useCallback((id: LlmProviderId, customId?: string) => {
    // 'custom' names a KIND, so it always travels with the endpoint's id - either alone is
    // not a valid choice, and update_settings rejects a custom selection that names no
    // configured endpoint. Comparing the PAIR (not just the id) is what makes switching
    // between two custom endpoints register as a change.
    const nextCustomId = id === 'custom' ? customId ?? null : null
    // The override is scoped to the CHOSEN provider - src/llm/detect.rs clears cfg.model
    // when testing any other one, precisely so one CLI's model string can't leak into
    // another's --model. Switching provider therefore DROPS the staged model instead of
    // carrying e.g. a claude alias over to codex; returning to the saved provider restores
    // whatever is on disk.
    const nextModel = id === savedId ? savedModel : ''
    const backToSaved = id === savedId && (id !== 'custom' || nextCustomId === savedCustomId)
    setSaveStatus('idle')
    // Re-picking the saved provider clears the stage, so Save can't offer to write a
    // change that isn't one.
    setPending(backToSaved ? null : { id, customId: nextCustomId, model: nextModel })
  }, [savedId, savedCustomId, savedModel])

  // Staging a model follows the same rule as staging a provider: land it in `pending`, and
  // collapse back to "nothing staged" the moment the pair matches what is on disk, so Save
  // never offers to write a non-change.
  const onModelChange = useCallback((next: string) => {
    setSaveStatus('idle')
    setPending(prev => {
      const staged = { ...(prev ?? { id: savedId, customId: savedCustomId, model: savedModel }), model: next }
      const settled = staged.id === savedId
        && (staged.id !== 'custom' || staged.customId === savedCustomId)
        && staged.model === savedModel
      return settled ? null : staged
    })
  }, [savedId, savedCustomId, savedModel])

  const onSave = useCallback(() => {
    if (!pending) return
    // A custom endpoint's model lives on the endpoint row, so the shared override is not
    // written for it. '' means "the provider's own default" and is stored as null, matching
    // `Option<String>`/`None` in meridian-core/src/settings.rs.
    const fields: Partial<RuntimeSettings> = pending.id === 'custom'
      ? { llm_provider: pending.id, llm_provider_custom_id: pending.customId }
      : { llm_provider: pending.id, llm_provider_model: pending.model || null }
    // `save` never rejects — it reports through the status callback — so the staged pick
    // is cleared on 'saved' only. On 'error' it deliberately survives, so the user can hit
    // Save again without re-picking.
    save(fields, (s) => {
      setSaveStatus(s)
      if (s === 'saved') setPending(null)
    })
  }, [pending, save])

  // Wider than the other sections' 640px: this one carries a card GRID, and the extra
  // width buys a third column (see the picker's auto-fill), which is what removes a whole
  // row of scrolling. Still inside the 980px modal's ~916px of usable width.
  return (
    <div className="max-w-[880px] flex flex-col gap-5">
      <div>
        <p className="mt-label" style={{ color: 'var(--color-state-proposal)' }}>Your AI</p>
        <h1 className="mt-title-lg mt-1.5" style={{ color: 'var(--t-title)' }}>Intelligence</h1>
        <p className="mt-body-sm mt-2 max-w-[520px]" style={{ color: 'var(--t-muted)' }}>
          The AI that writes your hourly summaries. Use a coding-agent CLI you already pay for, or
          a custom endpoint on your own key. Pick one and hit Save - it takes effect from the next hour,
          with nothing to restart.
        </p>
      </div>

      <LlmProviderPicker
        value={provider}
        selectedCustomId={selectedCustomId}
        onChange={onChange}
        status={status}
        scanning={scanning}
        testingIds={testingIds}
        testOne={testOne}
        rescan={rescan}
      />

      {/* The model within the chosen provider. Net-new UI: `llm_provider_model` has existed
          on both sides of the wire for a while (meridian-core/src/settings.rs, consumed at
          src/llm/config.rs) but nothing ever wrote it, so the override was unreachable.
          Blank stays the default - most users never need this. */}
      <div className="flex flex-col gap-2">
        <p className="mt-label" style={{ color: 'var(--t-faint)' }}>MODEL</p>
        {showModelPicker ? (
          <>
            <ModelSelect
              value={model}
              onChange={onModelChange}
              models={picked.models}
              emptyLabel={`${picked.name} default`}
            />
            <p className="mt-body-sm max-w-[520px]" style={{ color: 'var(--t-muted)' }}>
              Leave this on the default unless you have a reason not to - Meridian follows
              whatever {picked.name} would pick for itself. A model this list doesn't carry
              can be typed in; nothing here is a fixed set.
            </p>
          </>
        ) : picked.kind === 'custom' ? (
          <p className="mt-body-sm max-w-[520px]" style={{ color: 'var(--t-muted)' }}>
            A custom endpoint carries its own model - change it on the endpoint itself, above.
          </p>
        ) : (
          <ModelUnsupportedNote providerName={picked.name} />
        )}
      </div>

      {/* A KNOWN failure, from the last real test. Since the on-device fallback was
          removed there is nothing to degrade to, so this is the one thing to tell the
          user: the affected hours are deferred, not lost. A failed SAVE is reported by
          the sticky footer below, not here. */}
      {selectedBroken && selectedTest && selectedTest.outcome.status !== 'ok' && (
        <div className="rounded-xl p-3.5 flex items-start gap-2.5"
          style={{ border: '1px solid var(--status-error-dot)', background: 'color-mix(in srgb, var(--status-error-dot) 7%, transparent)' }}>
          <span className="shrink-0" style={{ marginTop: 2, color: 'var(--status-error-dot)' }} aria-hidden="true">⚠</span>
          <p className="mt-body-sm" style={{ color: 'var(--t-muted)' }}>
            {selectedTest.outcome.status === 'rate_limited'
              ? `${picked.name} is currently rate-limited: ${selectedTest.outcome.message}. `
              : `${picked.name} isn't responding right now: ${selectedTest.outcome.message}. `}
            Those hours are being held until this clears, then picked up automatically.
          </p>
        </div>
      )}

      {/* Save is offered only while a pick is staged, so a settled panel carries no dead
          control - and it is STICKY, pinned to the bottom of the Settings scroll area, so
          it can never fall below the fold however many providers, custom endpoints or
          warnings are on screen. Rendered last (after the warnings) so, once scrolled to
          the end, it sits in natural document order rather than overlapping anything.
          <SaveButton> supplies its own 'Saved'/'Failed to save' status, the same as
          Advanced/Capture/Notifications; the line above it carries the part that generic
          status can't - WHICH provider is staged, and (on failure) that the pick survived
          the failed write, so Save can simply be pressed again without re-picking. */}
      {pending && (
        <div className="sticky bottom-0 flex flex-col gap-2 pb-1"
          style={{ background: 'var(--t-panel)' }}>
          <p className="mt-body-sm" style={{
            color: saveStatus === 'error' ? 'var(--status-error-dot)' : 'var(--color-state-pending)',
          }}>
            {saveStatus === 'error'
              ? `Couldn't switch to ${llmProvider(pending.id).name} - it's still selected here, so you can press Save to try again.`
              : `${llmProvider(pending.id).name} isn't in use yet - Save to switch to it.`}
          </p>
          <SaveButton status={saveStatus} onClick={onSave} />
        </div>
      )}
    </div>
  )
}
