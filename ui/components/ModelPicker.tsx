//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

/**
 * The model-selection controls, shared by every surface that used to make the user TYPE a
 * model name: the Lab's variant cards, Intelligence settings, and the custom-endpoint form.
 *
 * # Two components, on purpose
 *
 * [`ModelSelect`] picks ONE model. [`ModelMultiSelect`] edits the Lab's comma-separated
 * fan-out list, where each model becomes its own variant column. They share this file and the
 * registry in `@/lib/llm-providers`, but not a component: collapsing them would mean one
 * control with a `multi` flag toggling both its markup and its value type, which is more
 * abstraction than two short components are worth.
 *
 * # Free text is never taken away
 *
 * Nothing in the pipeline validates a model string - it is passed verbatim into the CLI's
 * argv (`src/llm/claude.rs` and friends) or an endpoint's JSON body - so the curated lists
 * here are a convenience, not a whitelist. Both controls always keep a way to type a model we
 * have not listed, because a stale list must never block a user from a model that works. This
 * matters most for `cursor`, whose curated list is deliberately empty.
 *
 * # Who calls this
 *
 * `RunComposer` (multi), `IntelligenceSection` (single), `CustomProviders` (single).
 */

import { useId } from 'react'
import type { LlmModelOption } from '@/lib/llm-providers'

/** The sentinel `<option>` value meaning "the user wants to type their own". */
const FREE_TEXT = '__free__'

interface ModelSelectProps {
  /** The current model string. Empty means "no override" - see `emptyLabel`. */
  value: string
  onChange: (next: string) => void
  /** Curated options. May be empty, in which case this degrades to a plain text field. */
  models: LlmModelOption[]
  /**
   * What an empty value means to THIS caller - "Provider default" in settings, where blank is
   * legitimate, vs a prompt to choose where a model is required.
   */
  emptyLabel?: string
  /** Set when a model is mandatory (a custom endpoint has no default to fall back to). */
  required?: boolean
  /** Rendered next to the control - the refresh affordance for live-listed endpoints. */
  action?: React.ReactNode
  /** Replaces the control with a disabled note (a live listing in flight, say). */
  busyLabel?: string | null
}

/**
 * A single-model control: a dropdown over `models`, plus a "Custom model…" escape hatch that
 * swaps in a free-text field.
 *
 * The escape hatch is also what renders when `models` is empty or when `value` is a model we
 * do not list - so a user who has already stored an unlisted model keeps seeing it, rather
 * than having it silently snap to something else on their next visit.
 */
export function ModelSelect({
  value,
  onChange,
  models,
  emptyLabel = 'Provider default',
  required = false,
  action,
  busyLabel = null,
}: ModelSelectProps) {
  const id = useId()
  const known = models.some(m => m.id === value)
  // Free text whenever we have nothing to offer, or the stored value isn't in what we offer.
  const freeText = models.length === 0 || (value !== '' && !known)
  const selected = freeText ? FREE_TEXT : value
  const note = models.find(m => m.id === value)?.note

  if (busyLabel) {
    return (
      <p className="mt-body-sm" style={{ color: 'var(--t-faint)' }}>
        {busyLabel}
      </p>
    )
  }

  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-center gap-2">
        <select
          id={id}
          value={selected}
          onChange={e => {
            // Switching TO free text clears the value so the field starts empty and the
            // control doesn't flip straight back to the dropdown on the next render.
            onChange(e.target.value === FREE_TEXT ? '' : e.target.value)
          }}
          className="rounded-lg px-2.5 py-1.5 bg-ctrl min-w-0"
          style={{
            border: '1px solid var(--t-ctrl-border)',
            color: 'var(--t-title)',
            font: '500 12px var(--font-sans)',
            cursor: 'pointer',
          }}
        >
          {required ? (
            // A required field with nothing chosen still needs an option matching the empty
            // value, or the browser renders the FIRST model while the value is actually ''
            // - the control would claim a model the form isn't holding. Disabled, so it can
            // be shown but not re-selected.
            value === '' && <option value="" disabled>Select a model…</option>
          ) : (
            <option value="">{emptyLabel}</option>
          )}
          {models.map(m => (
            <option key={m.id} value={m.id}>
              {m.label}
            </option>
          ))}
          <option value={FREE_TEXT}>Custom model…</option>
        </select>
        {action}
      </div>

      {freeText && (
        <input
          type="text"
          value={value}
          onChange={e => onChange(e.target.value)}
          placeholder="model id, e.g. gemini-flash-latest"
          className="w-full rounded-lg px-2 py-1 bg-ctrl"
          style={{
            border: '1px solid var(--t-ctrl-border)',
            color: 'var(--t-title)',
            font: '500 11px var(--font-mono, ui-monospace)',
          }}
        />
      )}
      {note && (
        <p className="mt-body-sm" style={{ color: 'var(--t-faint)' }}>
          {note}
        </p>
      )}
    </div>
  )
}

interface ModelMultiSelectProps {
  /**
   * The raw comma-separated list, kept as a STRING rather than an array so the Lab's variant
   * assembly (`RunComposer`) keeps splitting it exactly as it did when this was a text input.
   */
  value: string
  onChange: (next: string) => void
  models: LlmModelOption[]
}

/** Split the stored comma list into trimmed, non-empty model ids. */
function parseList(value: string): string[] {
  return value
    .split(',')
    .map(m => m.trim())
    .filter(Boolean)
}

/**
 * The Lab's fan-out control: tick any number of curated models, each of which becomes its own
 * variant column, and still type anything else alongside them.
 *
 * Empty means "the provider's default model, one variant" - which is why nothing is ticked by
 * default and why the free-text field stays: the Lab's whole point is comparing models we may
 * not have listed yet.
 */
export function ModelMultiSelect({ value, onChange, models }: ModelMultiSelectProps) {
  const picked = parseList(value)

  function toggle(id: string) {
    const next = picked.includes(id) ? picked.filter(m => m !== id) : [...picked, id]
    onChange(next.join(', '))
  }

  return (
    <div className="mt-2 flex flex-col gap-1.5">
      {models.length > 0 && (
        <div className="flex flex-wrap gap-1">
          {models.map(m => {
            const on = picked.includes(m.id)
            return (
              <button
                key={m.id}
                type="button"
                onClick={() => toggle(m.id)}
                title={m.note ?? m.id}
                className="rounded-full px-2 py-0.5"
                style={{
                  border: `1px solid ${on ? 'var(--btn-primary-bg)' : 'var(--t-ctrl-border)'}`,
                  background: on ? 'var(--btn-primary-bg)' : 'var(--t-ctrl)',
                  color: on ? '#fff' : 'var(--t-muted)',
                  font: '600 10.5px var(--font-sans)',
                  cursor: 'pointer',
                }}
              >
                {m.label}
              </button>
            )
          })}
        </div>
      )}
      <input
        type="text"
        placeholder={
          models.length > 0
            ? 'or type model(s), comma-separated'
            : 'model(s), comma-separated (optional)'
        }
        title="Empty = the provider's default model. Several models = one variant each."
        value={value}
        onChange={e => onChange(e.target.value)}
        className="w-full rounded-lg px-2 py-1 bg-ctrl"
        style={{
          border: '1px solid var(--t-ctrl-border)',
          color: 'var(--t-title)',
          font: '500 11px var(--font-mono, ui-monospace)',
        }}
      />
    </div>
  )
}

/**
 * What we show instead of a picker for a provider whose backend discards the model.
 *
 * Only `copilot` today: its argv is built without a model flag and it never reads `cfg.model`
 * (`src/llm/copilot.rs`). A dropdown here would write a setting nothing reads - strictly worse
 * than saying so.
 */
export function ModelUnsupportedNote({ providerName }: { providerName: string }) {
  return (
    <p className="mt-body-sm" style={{ color: 'var(--t-faint)' }}>
      Model is managed by the {providerName} CLI - Meridian uses whichever model it is signed
      in to.
    </p>
  )
}
