//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// Connect ANY OpenAI-compatible endpoint - a local model or a cloud key - from a base URL.
//
// The advanced sibling of <CloudKeySetup>. That one is a walkthrough for someone who said
// they have no subscription and wants a working key in three clicks; this one is a form for
// someone who already has an endpoint in mind and needs to describe it. Keeping them apart
// is deliberate and is the whole reason the deleted add-form's rationale still holds: the
// no-subscription path never shows this, and the picker only offers it behind `!gate`.
//
// # Only what the user can actually tell us
//
// Three fields, and no more. The old form's vendor dropdown is gone (the vendor is always
// `custom` - see `CUSTOM_ENDPOINT_VENDOR`) and so are its two rate-limit boxes: nobody can
// state the RPM of a server we have never seen, and a guessed number is worse than `0`,
// which `meridian_core::llm_capacity::assess` reads as "not known" rather than "unmetered".
//
// # The API key is optional, and that is the point
//
// A local server (Ollama, LM Studio, llama.cpp, vLLM) has no auth, so there is no key to
// type. Rust permits a blank one at the door (`validate_transport_inputs`) and then sends no
// `Authorization` header at all (`openai_compat::with_auth`). A blank key against an endpoint
// that DOES want one fails here, at setup, with the server's own 401 - not hours later.
//
// # The model comes from the endpoint, and can still be typed
//
// `/models` is asked first, because a mistyped model id is the one error that would not
// surface until the first real hour. But plenty of OpenAI-compatible servers do not implement
// `/models` at all, so a failed listing degrades to a text field rather than blocking - the
// same degradation `openai_compat::list_models` documents for its callers.
//
// # Who calls this
// [`LlmProviderPicker`], for the `custom` vendor tile - never from the gate.
//
// # Related
// - `@/lib/llm-providers` - `CUSTOM_ENDPOINT_VENDOR`, `chatCapableModels`, `rungLabel`
// - `@/components/CloudKeySetup` - the curated-preset path this deliberately does not merge with
// - `@/components/CustomProviders` - the registry hook this writes through

import { BackLink } from '@/components/ui/BackLink'
import { useState } from 'react'
import { invoke } from '@/lib/bridge'
import {
  chatCapableModels, CUSTOM_ENDPOINT_VENDOR, rungLabel, type CustomProviderView,
  type ProbeOutcome,
} from '@/lib/llm-providers'

/** The registry write, shared with `<CloudKeySetup>` - `useCustomProviders().add`. */
type AddFn = (fields: {
  vendor: string; name: string; base_url: string; model: string
  api_key: string; rpm: number; rpd: number
}) => Promise<ProbeOutcome>

type Phase = 'idle' | 'listing' | 'adding' | 'selecting' | 'done'

/** A default name from the URL's host, so the common case needs no thought.
 *
 *  Host and port only: two LM Studio instances on different ports are different endpoints and
 *  must not collide, but the path segment (`/v1`) is noise in a name. Falls back to the raw
 *  string when `URL` cannot parse it - the field is editable and Rust validates the URL
 *  properly, so a bad parse here must never block typing. */
function nameFromUrl(raw: string): string {
  try {
    return new URL(raw).host
  } catch {
    return raw.replace(/^https?:\/\//, '').split('/')[0] ?? ''
  }
}

export default function CustomEndpointSetup({ existing, onBack, onAdd, onPick }: {
  /** Every endpoint already configured - used ONLY to keep the name unique up front. The
   *  tray rejects a duplicate name permanently, and it does so AFTER the probe has already
   *  been paid for, so catching it before the request is the difference between a retryable
   *  form and a dead end. */
  existing: CustomProviderView[]
  onBack: () => void
  onAdd: AddFn
  /** Make the freshly added endpoint the live provider. Rejects if the tray refuses. */
  onPick: (customId: string) => void | Promise<void>
}) {
  const [baseUrl, setBaseUrl] = useState('')
  const [apiKey, setApiKey] = useState('')
  const [model, setModel] = useState('')
  const [name, setName] = useState('')
  const [nameEdited, setNameEdited] = useState(false)
  const [models, setModels] = useState<string[] | null>(null)
  const [listError, setListError] = useState<string | null>(null)
  const [phase, setPhase] = useState<Phase>('idle')
  const [error, setError] = useState<string | null>(null)
  // Set once the endpoint EXISTS daemon-side. From here a retry must RE-MEASURE rather than
  // re-add: the tray refuses a duplicate name forever, so a second add can never succeed and
  // the screen would be a dead end. Same failure `<CloudKeySetup>` is shaped around.
  const [addedId, setAddedId] = useState<string | null>(null)

  const busy = phase === 'listing' || phase === 'adding' || phase === 'selecting'
  const url = baseUrl.trim()
  const effectiveName = (name.trim() || nameFromUrl(url)).trim()
  const canList = !busy && !!url
  // Once the endpoint is SAVED the button must stay live on the fields alone being empty:
  // the retry path re-measures a row the daemon already holds, so it needs no URL, key or
  // model from the form. Gating it on the form fields is what made `<CloudKeySetup>`'s
  // equivalent a dead end after a rate-limited probe.
  const canConnect = !busy && (addedId !== null || (!!url && !!model.trim() && !!effectiveName))

  /** Ask the endpoint what it serves. Doubles as the first real check that the URL (and the
   *  key, if the server wants one) work at all - before anything is written to settings. */
  async function listModels() {
    if (!canList) return
    setListError(null)
    setError(null)
    setPhase('listing')
    try {
      const ids = await invoke<string[]>('list_custom_llm_provider_models', {
        baseUrl: url,
        apiKey: apiKey.trim(),
      })
      const usable = chatCapableModels(ids)
      setModels(usable)
      // Only preselect when there is no ambiguity. Picking the first of many would put a
      // model in the field the user never chose, and a wrong model fails at the first real
      // hour rather than here.
      if (usable.length === 1) setModel(usable[0])
      if (usable.length === 0) {
        setListError(
          ids.length > 0
            ? 'This endpoint listed only non-chat models (embeddings, speech). Type a model id below if you know one it can chat with.'
            : 'This endpoint listed no models. Type the model id below.',
        )
      }
    } catch (e) {
      // NOT fatal. Many OpenAI-compatible servers do not implement /models, and refusing to
      // continue would make a working endpoint unconfigurable over an optional convenience.
      setModels([])
      setListError(
        `${String(e).replace(/^Error:\s*/, '')} - type the model id below if you know it.`,
      )
    } finally {
      setPhase('idle')
    }
  }

  async function connect() {
    if (busy) return
    setError(null)
    try {
      let outcome: ProbeOutcome
      if (addedId) {
        // RETRY, endpoint already saved: re-measure the row rather than create a second one.
        setPhase('adding')
        outcome = await invoke<ProbeOutcome>('probe_custom_llm_provider', {
          id: addedId, refresh: true,
        })
      } else {
        if (!model.trim()) throw new Error('Choose or type a model first.')
        if (existing.some((p) => p.name.toLowerCase() === effectiveName.toLowerCase())) {
          throw new Error(`You already have an endpoint named "${effectiveName}". Give this one a different name.`)
        }
        setPhase('adding')
        outcome = await onAdd({
          vendor: CUSTOM_ENDPOINT_VENDOR,
          name: effectiveName,
          base_url: url,
          model: model.trim(),
          api_key: apiKey.trim(),
          // `0/0` is "not known", NOT "unmetered" - see `llm_capacity::assess`. There is no
          // published limit to read for an endpoint the user brought, and inventing one would
          // produce a confident verdict about a quota nobody measured.
          rpm: 0, rpd: 0,
        })
        // Recorded BEFORE the eligibility check: the row exists now however that check goes.
        setAddedId(outcome.provider.id)
      }

      if (!outcome.provider.production_eligible) {
        // Say WHICH limitation, not just "failed". The gate wants shape-enforced JSON
        // (`SchemaRung::JsonSchema` or better) on all four pipeline schemas, and an endpoint
        // that answers fine in prose can still sit below it - most often a small local model.
        // Naming the measured rung is what makes this actionable rather than a dead button.
        const measured = rungLabel(outcome.provider.effective_rung)
        throw new Error(
          outcome.incomplete
            ? `Measuring stopped early: ${outcome.incomplete}. Press Connect again to finish.`
            : `${outcome.provider.model} answered, but the strongest structured-reply mode it ` +
              `held across all of Meridian's schemas was "${measured}". Meridian needs enforced ` +
              `JSON, so it cannot run the pipeline on this model. Try a larger or ` +
              `instruction-tuned model on the same endpoint - the endpoint is saved, so you ` +
              `can change its model and re-test from Settings.`,
        )
      }

      setPhase('selecting')
      await onPick(outcome.provider.id)
      setApiKey('')  // stored daemon-side now, and it never comes back
      setPhase('done')
    } catch (e) {
      setError(String(e).replace(/^Error:\s*/, ''))
      setPhase('idle')
    }
  }

  if (phase === 'done') {
    return (
      <div className="flex flex-col mer-pop" style={{ gap: 10, alignItems: 'flex-start' }}>
        <span className="font-mono self-start" style={{
          fontSize: 9, letterSpacing: '.1em', color: 'var(--color-state-approved)',
          border: '1px solid var(--color-state-approved)',
          background: 'color-mix(in srgb, var(--color-state-approved) 10%, transparent)',
          borderRadius: 4, padding: '2px 7px',
        }}>CONNECTED</span>
        <span style={{ fontSize: 17, fontWeight: 600, color: 'var(--t-title)' }}>
          You&apos;re set up
        </span>
        <p style={{ fontSize: 12, lineHeight: 1.5, color: 'var(--t-muted)', maxWidth: 480 }}>
          Meridian is using {effectiveName} ({model}) to write your summaries. You can change
          this any time in Settings.
        </p>
      </div>
    )
  }

  return (
    <div className="flex flex-col" style={{ gap: 14 }}>
      <BackLink onClick={onBack}>Back</BackLink>

      <div className="flex flex-col" style={{ gap: 5 }}>
        <span style={{ fontSize: 19, fontWeight: 600, color: 'var(--t-title)', letterSpacing: '-.01em' }}>
          Connect your own endpoint
        </span>
        <p style={{ fontSize: 12.5, lineHeight: 1.5, color: 'var(--t-muted)', maxWidth: 520 }}>
          Anything that speaks the OpenAI chat-completions API - Ollama or LM Studio on this
          machine, a server on your network, or a cloud provider on your own key.
        </p>
      </div>

      {/* Stated, not implied. A local endpoint is the reason most people are on this screen,
          and "does my screen data leave this machine" is the question it raises. The honest
          answer depends entirely on the URL they type, so this says exactly that rather than
          borrowing a preset's privacy badge for a server we know nothing about. */}
      <p style={{
        fontSize: 11.5, lineHeight: 1.45, color: 'var(--t-muted)',
        background: 'var(--t-box)', border: '1px solid var(--t-ctrl-border)',
        borderRadius: 9, padding: '9px 11px', maxWidth: 520,
      }}>
        Meridian sends one hour of activity at a time to whatever address you enter here. Point
        it at a model on this machine and nothing leaves it; point it at a cloud provider and
        their privacy policy applies, not ours.
      </p>

      <Step n={1} title="Where is it?"
        body="The base URL, including the version segment - Meridian adds /chat/completions itself.">
        <div className="flex flex-col" style={{ gap: 8 }}>
          <Field
            label="Base URL"
            value={baseUrl}
            onChange={(v) => {
              setBaseUrl(v)
              setModels(null)
              setListError(null)
              if (!nameEdited) setName('')
            }}
            placeholder="http://localhost:11434/v1"
            disabled={busy}
            mono
          />
          <Field
            label="API key"
            hint="Leave blank if your server does not need one - most local models do not."
            value={apiKey}
            onChange={setApiKey}
            placeholder="Optional"
            disabled={busy}
            password
            mono
          />
        </div>
      </Step>

      <Step n={2} title="Which model?"
        body="Meridian asks the endpoint what it serves. If it does not answer, type the id yourself.">
        <div className="flex flex-col" style={{ gap: 8 }}>
          <button
            onClick={listModels}
            disabled={!canList}
            className="self-start"
            style={{
              fontSize: 11.5, fontWeight: 600, color: 'var(--t-title)',
              padding: '6px 11px', borderRadius: 8,
              background: 'var(--t-ctrl)', border: '1px solid var(--t-ctrl-border)',
              opacity: canList ? 1 : 0.5,
              cursor: canList ? 'pointer' : 'default',
            }}>
            {phase === 'listing' ? 'Asking the endpoint…' : 'List models'}
          </button>

          {models && models.length > 0 && (
            <select
              value={model}
              onChange={(e) => setModel(e.target.value)}
              disabled={busy}
              style={{
                fontSize: 12, padding: '8px 10px', borderRadius: 8, width: '100%',
                border: '1px solid var(--t-ctrl-border)', background: 'var(--t-ctrl)',
                color: 'var(--t-title)', cursor: busy ? 'default' : 'pointer',
              }}>
              <option value="">Choose a model…</option>
              {models.map((m) => <option key={m} value={m}>{m}</option>)}
            </select>
          )}

          {/* Always available, never only as a fallback: a server may list a hundred models
              and the user may know exactly which one they want. */}
          <Field
            label="Model id"
            value={model}
            onChange={setModel}
            placeholder="llama3.1:8b"
            disabled={busy}
            mono
          />

          {listError && (
            <p style={{ fontSize: 11, lineHeight: 1.45, color: 'var(--t-muted)' }}>{listError}</p>
          )}
        </div>
      </Step>

      <Step n={3} title="Name it and connect"
        body="Meridian checks the endpoint can return the structured replies the pipeline needs.">
        <div className="flex flex-col" style={{ gap: 8 }}>
          <Field
            label="Name"
            value={name}
            onChange={(v) => { setName(v); setNameEdited(true) }}
            placeholder={nameFromUrl(url) || 'My endpoint'}
            disabled={busy}
          />
          <button
            onClick={connect}
            disabled={!canConnect}
            className="self-start"
            style={{
              fontSize: 12, fontWeight: 700, color: '#fff', padding: '8px 14px',
              borderRadius: 9, border: 'none',
              background: 'var(--btn-primary-bg)',
              opacity: canConnect ? 1 : 0.5,
              cursor: canConnect ? 'pointer' : 'default',
            }}>
            {phase === 'adding' ? 'Testing the endpoint…'
              : phase === 'selecting' ? 'Almost there…'
                : addedId ? 'Re-test and connect' : 'Connect'}
          </button>
          {/* Named, because this step runs real requests and can sit for a while. */}
          {phase === 'adding' && (
            <span style={{ fontSize: 10.5, lineHeight: 1.4, color: 'var(--t-faint)' }}>
              Checking which replies {model} can give - this takes a few seconds.
            </span>
          )}
          {error && (
            <p style={{
              fontSize: 11.5, lineHeight: 1.45, color: 'var(--color-state-pending)',
              background: 'color-mix(in srgb, var(--color-state-pending) 10%, transparent)',
              borderRadius: 8, padding: '8px 10px',
            }}>{error}</p>
          )}
        </div>
      </Step>
    </div>
  )
}

/** A labelled text input. Local to this file: the shape is specific to this form, and the
 *  repo has no shared field primitive to reach for. */
function Field({ label, hint, value, onChange, placeholder, disabled, mono, password }: {
  label: string
  hint?: string
  value: string
  onChange: (v: string) => void
  placeholder: string
  disabled: boolean
  mono?: boolean
  password?: boolean
}) {
  return (
    <label className="flex flex-col" style={{ gap: 4 }}>
      <span className="font-mono" style={{
        fontSize: 9, letterSpacing: '.09em', color: 'var(--t-faint)',
      }}>{label.toUpperCase()}</span>
      <input
        type={password ? 'password' : 'text'}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        spellCheck={false}
        autoComplete="off"
        disabled={disabled}
        style={{
          fontSize: 12, padding: '8px 10px', borderRadius: 8, width: '100%',
          border: '1px solid var(--t-ctrl-border)', background: 'var(--t-ctrl)',
          color: 'var(--t-title)',
          fontFamily: mono ? 'var(--font-mono, ui-monospace)' : undefined,
          opacity: disabled ? 0.6 : 1,
        }}
      />
      {hint && (
        <span style={{ fontSize: 10.5, lineHeight: 1.4, color: 'var(--t-faint)' }}>{hint}</span>
      )}
    </label>
  )
}

/** One numbered step - the same affordance `<CloudKeySetup>` uses, so the two screens read
 *  as the same product. Duplicated rather than shared because they are three lines of markup
 *  and lifting them into a shared module would couple two screens that are deliberately
 *  diverging. */
function Step({ n, title, body, children }: {
  n: number; title: string; body: string; children: React.ReactNode
}) {
  return (
    <div className="flex items-start" style={{ gap: 11 }}>
      <span className="flex items-center justify-center shrink-0 font-mono" style={{
        width: 22, height: 22, borderRadius: 999, fontSize: 10.5, fontWeight: 700,
        background: 'var(--t-box)', border: '1px solid var(--t-ctrl-border)',
        color: 'var(--t-title)', marginTop: 1,
      }}>{n}</span>
      <div className="flex flex-col min-w-0 flex-1" style={{ gap: 5 }}>
        <span style={{ fontSize: 13, fontWeight: 600, color: 'var(--t-title)' }}>{title}</span>
        <span style={{ fontSize: 11.5, lineHeight: 1.45, color: 'var(--t-muted)' }}>{body}</span>
        {children}
      </div>
    </div>
  )
}
