//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The custom-endpoint half of the AI-provider choice: the registry hook and the per-endpoint
// card. Split out of <LlmProviderPicker> purely for size - the picker owns the grid and the
// selection, this owns everything specific to a user-configured endpoint.
//
// THERE IS NO ADD FORM HERE ANY MORE. <CloudKeySetup> is the one way an endpoint gets
// created, from a single pasted key against one of the curated presets in
// `@/lib/llm-providers` (`CLOUD_PRESETS`); the old form asked for a vendor from an open-ended
// dropdown, a URL it filled in itself, a model from an unrankable list and two rate limits we
// now know for the presets that publish them. The vendor-preset machinery it stood on is gone
// too - see the note where it used to live in `@/lib/llm-providers`. `add` stays on the hook,
// because CloudKeySetup calls it.
//
// Two rules run through the whole file, and both come from Rust:
//
//  1. The API key is write-only. It goes out through `add_custom_llm_provider` and never
//     comes back - `CustomProviderView` has no key field at all. Nothing here stores one
//     beyond the in-flight form state.
//  2. The gate is NOT re-derived here. `production_eligible` / `fully_probed` are computed
//     in meridian-core and carried on the view; `update_settings` REJECTS a write that
//     selects an ineligible endpoint, so the UI must not offer one as selectable - a
//     rejected save would silently roll the choice back with nothing to explain it.

import { useCallback, useEffect, useState } from 'react'
import { invoke } from '@/lib/bridge'
import {
  rungLabel,
  capacityNotice,
  type CustomProviderView,
  type ProbeOutcome,
} from '@/lib/llm-providers'
import { CustomVendorLogo } from '@/components/LlmProviderLogos'

/**
 * The configured endpoints, plus the writes that change them.
 *
 * `list` is free (a settings.json read). `add` and `probe` are NOT: they spend real metered
 * requests against the user's own key, which is why neither ever runs on mount - only on an
 * explicit click.
 */
export function useCustomProviders() {
  const [providers, setProviders] = useState<CustomProviderView[]>([])
  const [loading, setLoading] = useState(true)
  /** Endpoint ids with a probe in flight. */
  const [probingIds, setProbingIds] = useState<Set<string>>(new Set())

  const refresh = useCallback(async () => {
    try {
      setProviders(await invoke<CustomProviderView[]>('list_custom_llm_providers'))
    } catch {
      // An unreadable registry renders as "none configured" rather than blocking the panel -
      // the built-in providers are still pickable, which is the important part.
      setProviders([])
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => { refresh() }, [refresh])

  /** Add + measure one endpoint. Throws the tray's message for the form to show verbatim. */
  const add = useCallback(async (fields: {
    vendor: string; name: string; base_url: string; model: string; api_key: string; rpm: number; rpd: number
  }): Promise<ProbeOutcome> => {
    // Tauri maps the Rust command's snake_case params to camelCase JS keys, so the invoke
    // args must be camelCase - `base_url`/`api_key` would be rejected as missing `baseUrl`.
    const outcome = await invoke<ProbeOutcome>('add_custom_llm_provider', {
      vendor: fields.vendor,
      name: fields.name,
      baseUrl: fields.base_url,
      model: fields.model,
      apiKey: fields.api_key,
      rpm: fields.rpm,
      rpd: fields.rpd,
    })
    await refresh()
    return outcome
  }, [refresh])

  /** Re-measure one endpoint - resumes what a rate limit cut short, or `refresh` re-measures
   *  from scratch when the endpoint's support may have changed under us. */
  const probe = useCallback(async (id: string, hard = false): Promise<ProbeOutcome | null> => {
    setProbingIds((prev) => new Set(prev).add(id))
    try {
      const outcome = await invoke<ProbeOutcome>('probe_custom_llm_provider', { id, refresh: hard })
      await refresh()
      return outcome
    } catch {
      return null
    } finally {
      setProbingIds((prev) => {
        const next = new Set(prev)
        next.delete(id)
        return next
      })
    }
  }, [refresh])

  /** Swap the key on an existing endpoint and re-measure it.
   *
   *  This, not remove-then-add, is how a rotated or mistyped key is fixed: `remove` is
   *  refused by the tray while the endpoint is the selected provider - which is precisely
   *  when a broken key matters - and re-adding hits the duplicate-name check. Shares the
   *  probing spinner because it re-probes on the new key. */
  const replaceKey = useCallback(async (id: string, apiKey: string): Promise<ProbeOutcome> => {
    setProbingIds((prev) => new Set(prev).add(id))
    try {
      // camelCase per the note in `add`.
      const outcome = await invoke<ProbeOutcome>('replace_custom_llm_provider_key', { id, apiKey })
      await refresh()
      return outcome
    } finally {
      setProbingIds((prev) => {
        const next = new Set(prev)
        next.delete(id)
        return next
      })
    }
  }, [refresh])

  return { providers, loading, probingIds, refresh, add, probe, replaceKey }
}

/**
 * One configured endpoint, as a single row.
 *
 * Rewritten from a dense tile that showed everything it knew at once - a status pill, the
 * model, a rung label, a quota notice and two filled buttons, all at 10px in a 232px column.
 * Every fact was true and none of them was ranked, so the card read as diagnostics rather
 * than as a thing you own. What a person needs here is: which endpoint, is it live, and the
 * two ways to act on it. The rest appears only when it is a PROBLEM.
 *
 * `selectable` is FALSE until meridian-core says the endpoint is production-eligible, and the
 * row then explains why rather than going quietly dead - an endpoint that was rate-limited
 * mid-probe is a Test click away from working, not broken.
 */
export function CustomProviderCard({ p, picked, live, statusDetail, probing, onPick, onProbe, onReplaceKey }: {
  p: CustomProviderView
  /** Selected in settings. A fact about settings.json, NOT about whether it works. */
  picked: boolean
  /** WHY it is not live, verbatim from the recorded test - "Disconnected from the dev panel",
   *  a 401 from the vendor, a spawn error. Passed in rather than guessed at: the card knows
   *  only that something is wrong, and a card that guesses writes sentences like "this key is
   *  not answering" over a key that was never asked. Undefined when `live`. */
  statusDetail?: string
  /** Selected AND the shared phase ladder says it is answering.
   *
   *  These are two different questions and this card used to ask only the first, so it said a
   *  confident green "In use" for an endpoint the chooser one screen back was calling NOT
   *  CONNECTED - the app disagreeing with itself about the same provider, one click apart.
   *  Computed by the caller from `phaseFor`, the same function the chooser tile, the detail
   *  screen and the Settings lock use. */
  live: boolean
  probing: boolean
  onPick: () => void
  onProbe: (hard: boolean) => void
  onReplaceKey: (apiKey: string) => Promise<unknown>
}) {
  // The inline key field, or null when closed. Empty string = open and untyped.
  const [newKey, setNewKey] = useState<string | null>(null)
  const [keyError, setKeyError] = useState<string | null>(null)
  const selectable = p.production_eligible
  const cardNotice = capacityNotice(p.capacity)
  // Only a PROBLEM is worth a line of prose. "JSON enforced (strict)" is the expected
  // outcome of a successful setup, and stating the expected outcome on every render is how
  // a settings screen turns into a status console.
  const problem = !selectable
    ? (!p.fully_probed
        ? 'Not fully tested yet - press Finish testing.'
        : 'This endpoint cannot hold Meridian to a JSON shape, so the pipeline cannot rely on it.')
    // Selected, measured, and not live. This state had NO route out: the card said "Not
    // connected" and offered two controls, neither of which named itself as the fix.
    //
    // The reason is REPORTED, not inferred. The key here may be perfectly good - the most
    // common way to reach this state is the dev Disconnect button, which asserts a failure
    // against a provider that was working seconds earlier. Telling that user their key "is
    // not answering" is simply false, and it points them at the one fix (a new key) that
    // cannot help.
    : picked && !live
      ? statusDetail ?? 'Not connected. Press Connect to check it again.'
      : null

  async function saveKey(e: React.MouseEvent) {
    e.stopPropagation()
    if (!newKey?.trim()) return
    setKeyError(null)
    try {
      await onReplaceKey(newKey.trim())
      setNewKey(null)  // the key is stored daemon-side now and never comes back
    } catch (err) {
      setKeyError(String(err).replace(/^Error:\s*/, ''))
    }
  }

  return (
    <div
      role="button"
      tabIndex={selectable ? 0 : -1}
      aria-disabled={!selectable}
      onClick={() => selectable && !picked && onPick()}
      onKeyDown={(e) => { if (selectable && (e.key === 'Enter' || e.key === ' ')) { e.preventDefault(); onPick() } }}
      className="flex flex-col text-left"
      style={{
        gap: 10, padding: '14px 16px', borderRadius: 12,
        cursor: selectable && !picked ? 'pointer' : 'default',
        background: 'var(--t-card)',
        // The live row is marked by ONE hairline in the connected colour, not a tinted fill
        // and an inset ring. At this size a filled card competes with the page for attention
        // and there is nothing on the page for it to compete with.
        border: `1px solid ${live ? 'var(--color-state-approved)' : 'var(--t-ctrl-border)'}`,
        opacity: selectable ? 1 : 0.7,
      }}>
      <div className="flex items-center" style={{ gap: 11 }}>
        <span className="flex items-center justify-center shrink-0" style={{
          width: 30, height: 30, borderRadius: 8,
          background: 'var(--t-box)', border: '1px solid var(--t-ctrl-border)',
        }}>
          <CustomVendorLogo vendor={p.vendor} size={15} />
        </span>
        <div className="flex flex-col min-w-0" style={{ flex: 1, gap: 2 }}>
          <span style={{ fontSize: 14.5, fontWeight: 600, color: 'var(--t-title)', letterSpacing: '-.01em' }}>
            {p.name}
          </span>
          {/* Model and capability on ONE line. The model is the only identifying detail worth
              carrying, and the rung qualifies it rather than shouting.
              --t-muted, not --t-faint, and 12px rather than 10.5: this is the line that says
              WHICH model is writing your summaries. At the faint token it was decoration -
              legible in a screenshot, greyed out on a real display. Sentence case, not
              lowercased: forcing "JSON" to "json" made a proper noun look like a typo. */}
          <span className="font-mono truncate" style={{ fontSize: 12, color: 'var(--t-muted)' }}>
            {p.model}{selectable ? ` · ${rungLabel(p.effective_rung)}` : ''}
          </span>
        </div>
        {probing ? (
          <span className="font-mono shrink-0" style={{ fontSize: 10.5, letterSpacing: '.08em', color: 'var(--t-muted)' }}>
            TESTING…
          </span>
        ) : picked ? (
          <span className="flex items-center shrink-0" style={{
            gap: 6, fontSize: 12.5, fontWeight: 600,
            color: live ? 'var(--color-state-approved)' : 'var(--t-muted)',
          }}>
            <span style={{
              width: 7, height: 7, borderRadius: 99,
              background: live ? 'currentColor' : 'transparent',
              border: live ? 'none' : '1.5px solid currentColor',
            }} />
            {live ? 'In use' : 'Not connected'}
          </span>
        ) : selectable ? (
          <span className="shrink-0" style={{ fontSize: 12.5, fontWeight: 600, color: 'var(--t-accent)' }}>
            Use this
          </span>
        ) : null}
      </div>

      {problem && (
        <p style={{ fontSize: 12, lineHeight: 1.45, color: 'var(--color-state-pending)' }}>{problem}</p>
      )}
      {/* Quota verdict. Kept because a plan's limits can change under a key that was fine
          when it was added - but it is silent when there is nothing wrong. */}
      {cardNotice && cardNotice.tone !== 'info' && (
        <p style={{
          fontSize: 12, lineHeight: 1.45,
          color: cardNotice.tone === 'error' ? 'var(--status-error-dot)' : 'var(--color-state-pending)',
        }}>
          {cardNotice.text}
        </p>
      )}
      {keyError && <p style={{ fontSize: 12, lineHeight: 1.45, color: 'var(--status-error-dot)' }}>{keyError}</p>}

      {/* Quiet text actions. They were filled mono buttons shouting RE-TEST / REMOVE at the
          same weight as the endpoint's own name - two rarely-used maintenance controls
          outranking the thing they maintain. */}
      {/* NO REMOVE. It was refused by the tray whenever it was worth pressing - an endpoint
          cannot be removed while it is the selected provider, and this screen is reached
          through the tile that IS the selected provider - so the button's only reachable
          outcome was the red line "switch to another provider first". Changing the key is
          what people actually come here to do, and it used to be impossible: remove-then-add
          was blocked at the remove, and re-adding hit the duplicate-name check. */}
      {newKey === null ? (
        <div className="flex items-center" style={{ gap: 10 }}>
          {/* Filled while it is the way out of a dead card, quiet otherwise - the same
              control either way, weighted by whether anything needs doing. */}
          <button onClick={(e) => { e.stopPropagation(); onProbe(p.fully_probed) }}
            disabled={probing}
            style={problem && !probing
              ? {
                  fontSize: 12, fontWeight: 600, padding: '6px 12px', borderRadius: 8,
                  border: 'none', background: 'var(--btn-primary-bg)', color: '#fff',
                  cursor: 'pointer',
                }
              : {
                  fontSize: 12, fontWeight: 500, color: 'var(--t-muted)', background: 'none',
                  border: 'none', padding: 0, cursor: probing ? 'default' : 'pointer',
                  opacity: probing ? 0.5 : 1,
                }}>
            {!p.fully_probed ? 'Finish testing' : picked && !live ? 'Connect' : 'Test connection'}
          </button>
          <span aria-hidden style={{ width: 1, height: 11, background: 'var(--t-ctrl-border)' }} />
          <button onClick={(e) => { e.stopPropagation(); setKeyError(null); setNewKey('') }}
            disabled={probing}
            style={{
              fontSize: 12, fontWeight: 500, color: 'var(--t-muted)', background: 'none',
              border: 'none', padding: 0, cursor: 'pointer',
            }}>
            Change key
          </button>
        </div>
      ) : (
        <div className="flex items-center" style={{ gap: 8 }} onClick={(e) => e.stopPropagation()}>
          <input
            type="password"
            value={newKey}
            autoFocus
            onChange={(e) => setNewKey(e.target.value)}
            placeholder="Paste a new key"
            spellCheck={false}
            autoComplete="off"
            style={{
              flex: 1, minWidth: 0, fontSize: 12, padding: '7px 10px', borderRadius: 8,
              border: '1px solid var(--t-ctrl-border)', background: 'var(--t-ctrl)',
              color: 'var(--t-title)', fontFamily: 'var(--font-mono, ui-monospace)',
            }}
          />
          <button onClick={saveKey} disabled={probing || !newKey.trim()}
            style={{
              fontSize: 12, fontWeight: 600, padding: '7px 13px', borderRadius: 8, border: 'none',
              background: probing || !newKey.trim() ? 'var(--t-ctrl)' : 'var(--btn-primary-bg)',
              color: probing || !newKey.trim() ? 'var(--t-faint)' : '#fff',
              cursor: probing || !newKey.trim() ? 'default' : 'pointer', whiteSpace: 'nowrap',
            }}>
            {probing ? 'Checking…' : 'Save'}
          </button>
          <button onClick={(e) => { e.stopPropagation(); setNewKey(null); setKeyError(null) }}
            disabled={probing}
            style={{
              fontSize: 12, color: 'var(--t-muted)', background: 'none', border: 'none',
              padding: 0, cursor: 'pointer',
            }}>
            Cancel
          </button>
        </div>
      )}
    </div>
  )
}
