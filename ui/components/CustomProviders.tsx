//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The custom-endpoint half of the AI-provider choice: the registry hook, the add form, and
// the per-endpoint cards. Split out of <LlmProviderPicker> purely for size - the picker owns
// the grid and the selection, this owns everything specific to a user-configured endpoint.
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
import { invoke, openExternal } from '@/lib/bridge'
import {
  CUSTOM_PROVIDER_COST_NOTE,
  CUSTOM_VENDOR_PRESETS,
  customVendorPreset,
  rungLabel,
  type CustomProviderView,
  type ProbeOutcome,
} from '@/lib/llm-providers'

/**
 * The configured endpoints, plus the writes that change them.
 *
 * `list` is free (a settings.json read). `add` and `probe` are NOT: they spend real metered
 * requests against the user's own key, which is why neither ever runs on mount - only on an
 * explicit click. See CUSTOM_PROVIDER_COST_NOTE.
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
    vendor: string; name: string; base_url: string; model: string; api_key: string; rpm: number
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

  const remove = useCallback(async (id: string) => {
    // Refused by the tray while the endpoint is the selected provider - let that message
    // through rather than pre-judging it here, so the rule lives in one place.
    const rows = await invoke<CustomProviderView[]>('remove_custom_llm_provider', { id })
    setProviders(rows)
  }, [])

  return { providers, loading, probingIds, refresh, add, probe, remove }
}

function Field({ label, value, onChange, placeholder, type = 'text', mono }: {
  label: string; value: string; onChange: (v: string) => void
  placeholder?: string; type?: string; mono?: boolean
}) {
  return (
    <label className="flex flex-col" style={{ gap: 4 }}>
      <span className="font-mono" style={{ fontSize: 9, letterSpacing: '.1em', textTransform: 'uppercase', color: 'var(--t-faint)' }}>
        {label}
      </span>
      <input
        type={type}
        value={value}
        placeholder={placeholder}
        onChange={(e) => onChange(e.target.value)}
        spellCheck={false}
        autoComplete="off"
        style={{
          fontSize: 11.5, padding: '6px 8px', borderRadius: 7,
          border: '1px solid var(--t-ctrl-border)', background: 'var(--t-ctrl)',
          color: 'var(--t-title)', fontFamily: mono ? 'var(--font-mono, ui-monospace)' : 'inherit',
        }}
      />
    </label>
  )
}

/**
 * The add form. Adding MEASURES the endpoint (up to a handful of real requests), so the
 * button says so and the cost note sits directly above the key field - where the decision is
 * actually made, not buried in docs read after a billing-enabled key is already pasted.
 */
function AddForm({ onAdd, onCancel }: {
  onAdd: (fields: { vendor: string; name: string; base_url: string; model: string; api_key: string; rpm: number }) => Promise<ProbeOutcome>
  onCancel: () => void
}) {
  const [vendor, setVendor] = useState(CUSTOM_VENDOR_PRESETS[0].id)
  const [name, setName] = useState('')
  const [baseUrl, setBaseUrl] = useState(CUSTOM_VENDOR_PRESETS[0].baseUrl)
  const [model, setModel] = useState('')
  const [apiKey, setApiKey] = useState('')
  // Held as a string because the number input reports one; coerced at submit. Empty or
  // unparseable means 0, i.e. unpaced - the same as the field never being touched.
  const [rpm, setRpm] = useState('0')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  /** A probe that stopped early - the endpoint IS saved, so this is a "resume it" notice,
   *  not a failure. Keeping the form open to say so beats making the user infer it from a
   *  card that merely isn't fully tested. */
  const [incomplete, setIncomplete] = useState<string | null>(null)

  const preset = customVendorPreset(vendor)
  // Only the escape hatch types its own URL; a preset's URL is fixed so a typo can't turn
  // "Gemini" into an endpoint that isn't Gemini.
  const urlEditable = vendor === 'other'

  function pickVendor(id: string) {
    setVendor(id)
    const p = customVendorPreset(id)
    setBaseUrl(p?.baseUrl ?? '')
    if (!name.trim() && p && id !== 'other') setName(p.name)
  }

  async function submit() {
    setBusy(true)
    setError(null)
    setIncomplete(null)
    try {
      const outcome = await onAdd({ vendor, name, base_url: baseUrl, model, api_key: apiKey, rpm: Number(rpm) || 0 })
      setApiKey('')  // the key is stored daemon-side now and never comes back - don't keep it here
      // The endpoint is saved either way; only a COMPLETE measurement closes the form, since
      // an incomplete one has something the user needs to act on.
      if (outcome.incomplete) setIncomplete(outcome.incomplete)
      else onCancel()
    } catch (e) {
      setError(String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="flex flex-col" style={{
      gap: 10, padding: '13px 14px', borderRadius: 13,
      background: 'var(--t-box)', border: '1px solid var(--t-card-border)',
    }}>
      <label className="flex flex-col" style={{ gap: 4 }}>
        <span className="font-mono" style={{ fontSize: 9, letterSpacing: '.1em', textTransform: 'uppercase', color: 'var(--t-faint)' }}>
          Provider
        </span>
        <select value={vendor} onChange={(e) => pickVendor(e.target.value)}
          style={{
            fontSize: 11.5, padding: '6px 8px', borderRadius: 7,
            border: '1px solid var(--t-ctrl-border)', background: 'var(--t-ctrl)', color: 'var(--t-title)',
          }}>
          {CUSTOM_VENDOR_PRESETS.map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
        </select>
      </label>
      {preset?.hint && (
        <p style={{ fontSize: 10.5, lineHeight: 1.4, color: 'var(--t-faint)', marginTop: -4 }}>{preset.hint}</p>
      )}

      <Field label="Name" value={name} onChange={setName} placeholder="Gemini Flash" />
      {urlEditable && (
        <Field label="Base URL" value={baseUrl} onChange={setBaseUrl} mono
          placeholder="https://host/v1" />
      )}
      <Field label="Model" value={model} onChange={setModel} mono placeholder="gemini-flash-latest" />

      {/* The cost warning sits with the key field, because that is where the money decision
          is made - see CUSTOM_PROVIDER_COST_NOTE. */}
      <div className="flex items-start" style={{
        gap: 8, padding: '9px 10px', borderRadius: 9,
        background: 'color-mix(in srgb, var(--color-state-pending) 8%, transparent)',
        border: '0.5px solid var(--color-state-pending)',
      }}>
        <span className="shrink-0" style={{ marginTop: 1, color: 'var(--color-state-pending)' }} aria-hidden="true">△</span>
        <p style={{ fontSize: 10.5, lineHeight: 1.45, color: 'var(--t-muted)' }}>{CUSTOM_PROVIDER_COST_NOTE}</p>
      </div>

      <Field label="API key" value={apiKey} onChange={setApiKey} type="password" mono
        placeholder="pasted once - never shown again" />
      {preset?.keyUrl && (
        <button onClick={() => preset.keyUrl && openExternal(preset.keyUrl)}
          className="self-start"
          style={{ fontSize: 10.5, color: 'var(--color-state-proposal)', background: 'none', border: 'none', padding: 0, cursor: 'pointer', textDecoration: 'underline' }}>
          Get a {preset.name} key ↗
        </button>
      )}

      <Field label="Requests per minute" value={rpm} onChange={setRpm} type="number" mono
        placeholder="0 - no limit" />
      <p style={{ fontSize: 10.5, lineHeight: 1.45, color: 'var(--t-muted)' }}>
        Free plans usually cap this - Gemini and Groq publish it on their pricing page. Set it
        and Meridian spaces its requests to stay under the cap, including the burst it sends
        when measuring this endpoint. Leave 0 if your plan has no limit.
      </p>

      {error && <p style={{ fontSize: 10.5, lineHeight: 1.4, color: 'var(--status-error-dot)' }}>{error}</p>}

      {/* Saved, but not fully measured - say so plainly, because the card alone would only
          show the symptom (not selectable) and not the reason. */}
      {incomplete && (
        <p style={{ fontSize: 10.5, lineHeight: 1.45, color: 'var(--color-state-pending)' }}>
          Added, but testing stopped early: {incomplete}. It’s saved and usable in the LLM Lab -
          press Test on its card when the limit clears to finish measuring it, and it can then be
          selected here.
        </p>
      )}

      <div className="flex items-center" style={{ gap: 8 }}>
        <button onClick={submit} disabled={busy}
          style={{
            fontSize: 11, fontWeight: 600, padding: '5px 11px', borderRadius: 7, border: 'none',
            background: busy ? 'var(--t-ctrl)' : 'var(--btn-primary-bg)',
            color: busy ? 'var(--t-faint)' : '#fff', cursor: busy ? 'default' : 'pointer',
          }}>
          {busy ? 'Testing…' : 'Add and test'}
        </button>
        <button onClick={onCancel} disabled={busy}
          style={{ fontSize: 11, color: 'var(--t-faint)', background: 'none', border: 'none', cursor: 'pointer' }}>
          Cancel
        </button>
        <span style={{ fontSize: 10, color: 'var(--t-faint)' }}>
          Adding sends a few real requests to check what this endpoint supports.
        </span>
      </div>
    </div>
  )
}

/**
 * One configured endpoint.
 *
 * `selectable` is FALSE until meridian-core says the endpoint is production-eligible, and the
 * card then explains why rather than going quietly dead - an endpoint that was rate-limited
 * mid-probe is a Test click away from working, not broken.
 */
export function CustomProviderCard({ p, picked, probing, onPick, onProbe, onRemove }: {
  p: CustomProviderView
  picked: boolean
  probing: boolean
  onPick: () => void
  onProbe: (hard: boolean) => void
  onRemove: () => Promise<void>
}) {
  const [removeError, setRemoveError] = useState<string | null>(null)
  const selectable = p.production_eligible

  async function remove(e: React.MouseEvent) {
    e.stopPropagation()
    setRemoveError(null)
    try {
      await onRemove()
    } catch (err) {
      setRemoveError(String(err))
    }
  }

  return (
    <div
      role="button"
      tabIndex={selectable ? 0 : -1}
      aria-disabled={!selectable}
      onClick={() => selectable && onPick()}
      onKeyDown={(e) => { if (selectable && (e.key === 'Enter' || e.key === ' ')) { e.preventDefault(); onPick() } }}
      className="flex flex-col text-left h-full"
      style={{
        gap: 8, padding: '13px 14px', borderRadius: 13,
        cursor: selectable ? 'pointer' : 'default',
        // Surfaces match <ProviderCard> exactly - these tiles share its grid, so any
        // divergence reads as one of the two being broken. See the layering note there
        // (card on --t-card, inner elements on --t-box).
        background: picked
          ? 'color-mix(in srgb, var(--color-state-proposal) 12%, var(--t-card))'
          : 'var(--t-card)',
        border: `1px solid ${picked ? 'var(--color-state-proposal)' : 'var(--t-ctrl-border)'}`,
        boxShadow: picked
          ? 'inset 0 0 0 1px color-mix(in srgb, var(--color-state-proposal) 30%, transparent)'
          : '0 1px 2px rgba(0,0,0,.04)',
        opacity: selectable ? 1 : 0.72,
      }}>
      <div className="flex items-start" style={{ gap: 10 }}>
        <span className="flex items-center justify-center shrink-0" style={{
          width: 30, height: 30, borderRadius: 9,
          background: picked ? 'color-mix(in srgb, var(--color-state-proposal) 16%, transparent)' : 'var(--t-box)',
          border: '1px solid var(--t-ctrl-border)',
          color: picked ? 'var(--color-state-proposal)' : 'var(--t-muted)',
        }}>
          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">
            <path d="M17.5 19a4.5 4.5 0 0 0 .5-8.97A6 6 0 0 0 6.1 9.4 4.5 4.5 0 0 0 6.5 19z" />
          </svg>
        </span>
        <div style={{ flex: 1, minWidth: 0 }}>
          <span style={{ fontSize: 13.5, fontWeight: 500, color: 'var(--t-title)' }}>{p.name}</span>
          <div className="flex items-center flex-wrap" style={{ gap: 5, marginTop: 4 }}>
            {/* Sizing/weight kept in step with <Badge> in LlmProviderPicker. */}
            <span className="font-mono" style={{
              fontSize: 9.5, letterSpacing: '.09em',
              color: p.production_eligible ? 'var(--color-state-approved)' : 'var(--color-state-pending)',
              border: `1px solid ${p.production_eligible ? 'var(--color-state-approved)' : 'var(--color-state-pending)'}`,
              background: `color-mix(in srgb, ${p.production_eligible ? 'var(--color-state-approved)' : 'var(--color-state-pending)'} 10%, transparent)`,
              borderRadius: 4, padding: '1.5px 5px', whiteSpace: 'nowrap', textTransform: 'uppercase',
            }}>
              {probing ? 'TESTING…' : rungLabel(p.effective_rung)}
            </span>
          </div>
        </div>
        {selectable && (
          <span className="flex items-center justify-center shrink-0" style={{
            width: 17, height: 17, borderRadius: 99, marginTop: 1,
            border: `1.5px solid ${picked ? 'var(--color-state-proposal)' : 'var(--t-card-border)'}`,
            background: picked ? 'var(--color-state-proposal)' : 'transparent',
          }}>
            {picked && <span style={{ width: 6, height: 6, borderRadius: 99, background: '#fff' }} />}
          </span>
        )}
      </div>

      <p className="font-mono" style={{ fontSize: 10, lineHeight: 1.4, color: 'var(--t-muted)', wordBreak: 'break-all' }}>
        {p.model}
      </p>

      {/* Why it can't be selected - never a dead card with no explanation. */}
      {!selectable && (
        <p style={{ fontSize: 10.5, lineHeight: 1.4, color: 'var(--t-title)' }}>
          {!p.fully_probed
            ? 'Not fully tested yet - press Test to finish measuring it. Until then it can only be used in the LLM Lab.'
            : 'This endpoint can’t hold Meridian to a JSON shape, so the pipeline can’t rely on it. It stays available in the LLM Lab.'}
        </p>
      )}

      {removeError && <p style={{ fontSize: 10.5, lineHeight: 1.4, color: 'var(--status-error-dot)' }}>{removeError}</p>}

      <div className="flex items-center" style={{ gap: 6 }}>
        {/* Half-measured → resume (buys only the schemas still missing). Fully measured →
            a real re-measure, since what the endpoint supports can change under us. */}
        <button onClick={(e) => { e.stopPropagation(); onProbe(p.fully_probed) }}
          disabled={probing}
          className="font-mono"
          style={{
            // Filled controls, matching the Test button in <ProviderCard>.
            fontSize: 10, letterSpacing: '.08em', textTransform: 'uppercase',
            color: 'var(--t-title)', border: '1px solid var(--t-ctrl-border)',
            borderRadius: 5, padding: '4px 8px', background: 'var(--t-box)',
            cursor: probing ? 'default' : 'pointer', opacity: probing ? 0.55 : 1,
          }}>
          {probing ? 'Testing…' : p.fully_probed ? 'Re-test' : 'Test'}
        </button>
        <button onClick={remove} disabled={probing}
          className="font-mono"
          style={{
            fontSize: 10, letterSpacing: '.08em', textTransform: 'uppercase',
            color: 'var(--t-muted)', border: '1px solid var(--t-ctrl-border)',
            borderRadius: 5, padding: '4px 8px', background: 'var(--t-box)', cursor: 'pointer',
          }}>
          Remove
        </button>
      </div>
    </div>
  )
}

/** The "+ Add custom endpoint" tile, and the form it opens in place. */
export function AddCustomProvider({ onAdd }: {
  onAdd: (fields: { vendor: string; name: string; base_url: string; model: string; api_key: string; rpm: number }) => Promise<ProbeOutcome>
}) {
  const [open, setOpen] = useState(false)
  // The form is wide (URL, model, key), so it takes the whole grid row; the closed tile
  // is just one more cell. The span lives here because only this component knows which
  // of the two is showing.
  if (open) {
    return (
      <div style={{ gridColumn: '1 / -1' }}>
        <AddForm onAdd={onAdd} onCancel={() => setOpen(false)} />
      </div>
    )
  }
  return (
    <button onClick={() => setOpen(true)}
      className="flex items-center justify-center h-full"
      style={{
        // Same wrap as <ProviderCard>/<CustomProviderCard> - padding, radius, surface and
        // border weight all match, so it sits in the grid as a peer rather than an
        // afterthought. Only the border stays DASHED, which is what still reads it as an
        // "add" affordance instead of a selectable option.
        gap: 7, padding: '13px 14px', borderRadius: 13, minHeight: 76,
        background: 'var(--t-card)', border: '1px dashed var(--t-ctrl-border)',
        boxShadow: '0 1px 2px rgba(0,0,0,.04)',
        color: 'var(--t-muted)', cursor: 'pointer', fontSize: 12,
      }}>
      + Add a custom endpoint
    </button>
  )
}
