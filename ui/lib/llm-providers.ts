//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

// Single source of truth for the AI providers — the choice the user makes once in the
// setup wizard and can change afterwards in Settings. Consumed by BOTH surfaces through
// the shared <LlmProviderPicker> component, exactly as `integrations.ts`'s TRACKERS is
// shared between the wizard and the dashboard. One list, one place, no drift.
//
// The ids are the WIRE FORMS — they must match `LlmProvider::as_str()` in
// meridian-core/src/llm_provider.rs, because they are written verbatim into
// settings.json's `llm_provider` and parsed back by `LlmProvider::from_wire`.
// `update_settings` rejects anything not in this set.
//
// The four CLI providers run on the user's OWN subscription: Meridian shells out to a
// coding-agent CLI that is already installed and signed in. `custom` is a user-configured
// OpenAI-compatible cloud endpoint on the user's own key.

export type LlmProviderId = 'claude' | 'codex' | 'cursor' | 'copilot' | 'custom'

/**
 * How well an endpoint was MEASURED to honour a structured-output request (mirrors
 * `SchemaRung` in meridian-core/src/settings.rs, snake_case wire forms).
 *
 * Ordered worst to best. `json_schema` and above enforce the answer's SHAPE, which is what
 * the pipeline needs; below that a bad answer doesn't fail loudly, it drops an hour.
 */
export type SchemaRung = 'none' | 'prompt' | 'json_object' | 'json_schema' | 'strict'

/**
 * One configured custom endpoint as the tray reports it (mirrors `CustomProviderView` in
 * tray/src-tauri/src/commands/custom_llm.rs).
 *
 * There is no `api_key` field, deliberately: the key never leaves the daemon side. The
 * verdicts (`effective_rung`, `production_eligible`, `fully_probed`) are computed in Rust
 * and carried here so the gate lives in exactly one place - never re-derive them from
 * `rungs` in the UI.
 */
export interface CustomProviderView {
  id: string
  vendor: string
  name: string
  base_url: string
  model: string
  /** Requests-per-minute ceiling; 0 = unpaced. See `CustomLlmProvider::rpm` in Rust. */
  rpm: number
  /** Requests-per-day ceiling; 0 = not known (never "zero allowed"). */
  rpd: number
  /** Whether these limits can run the app for a day. Computed in Rust - never re-derive
   *  it here from rpm/rpd, or the two answers will drift. */
  capacity: CapacityAssessment
  rungs: Record<string, SchemaRung>
  effective_rung: SchemaRung
  fully_probed: boolean
  production_eligible: boolean
  selected: boolean
}

/** How an endpoint's configured quota compares with what the app actually asks for.
 *  Mirrors `meridian_core::llm_capacity::CapacityVerdict`. */
export type CapacityVerdict =
  | 'sufficient'
  | 'unknown'
  | 'tight'
  | 'insufficient'
  | 'cannot_onboard'

/** Mirrors `meridian_core::llm_capacity::CapacityAssessment`. */
export interface CapacityAssessment {
  verdict: CapacityVerdict
  /** Active hours/day this quota covers once setup is paid for; null when RPD is unknown. */
  covered_active_hours: number | null
  /** Requests a normal working day needs. */
  daily_demand: number
  /** Seconds the setup probe will take at this RPM, when low enough to be worth saying. */
  probe_seconds_at_rpm: number | null
}

/** What `add_custom_llm_provider` / `probe_custom_llm_provider` report back. */
export interface ProbeOutcome {
  provider: CustomProviderView
  /** Real metered requests that run spent. */
  requests: number
  /** Why the probe stopped early (usually a rate limit), or null if it completed. */
  incomplete: string | null
}

/** What to tell the user about an endpoint's quota, or null when there is nothing worth
 *  saying. Returns the severity so the caller can style it, and prose that states the
 *  working-day assumption rather than presenting the verdict as fact.
 *
 *  All hyphens here are plain `-` on purpose: this is user-facing app text. */
export function capacityNotice(
  c: CapacityAssessment,
): { tone: 'error' | 'warn' | 'info'; text: string } | null {
  const day = `about ${c.daily_demand} requests a day`
  switch (c.verdict) {
    case 'cannot_onboard':
      return {
        tone: 'error',
        text:
          `This key's daily limit is too small to even measure the endpoint - checking which ` +
          `JSON modes it supports costs up to 16 requests on its own. Pick a model with a ` +
          `higher requests-per-day allowance.`,
      }
    case 'insufficient':
      return {
        tone: 'error',
        text:
          `This key covers roughly ${c.covered_active_hours} active ${hours(c.covered_active_hours)} ` +
          `a day. Meridian needs ${day} for a normal 8-hour day, so hours past that will be ` +
          `skipped until the quota resets. Pick a model with a higher requests-per-day allowance.`,
      }
    case 'tight':
      return {
        tone: 'warn',
        text:
          `This key covers roughly ${c.covered_active_hours} active ${hours(c.covered_active_hours)} ` +
          `a day - enough for a short day, not a full one (Meridian needs ${day}). A model with a ` +
          `higher requests-per-day allowance would be safer.`,
      }
    case 'unknown':
      return {
        tone: 'info',
        text:
          `Add this plan's requests-per-day limit and Meridian can tell you whether it's enough ` +
          `for a full day - it needs ${day}. Free tiers are often capped low enough to matter.`,
      }
    case 'sufficient':
      return c.probe_seconds_at_rpm && c.probe_seconds_at_rpm > 60
        ? {
            tone: 'info',
            text:
              `Plenty for a day's work. The one-time setup check will take about ` +
              `${Math.round(c.probe_seconds_at_rpm / 60)} minutes, because requests are spaced out ` +
              `to stay under this key's per-minute limit.`,
          }
        : null
  }
}

function hours(n: number | null): string {
  return n === 1 ? 'hour' : 'hours'
}

/** Human label for a measured rung - what the card says about how far it can be trusted. */
export function rungLabel(rung: SchemaRung): string {
  switch (rung) {
    case 'strict':
      return 'JSON enforced (strict)'
    case 'json_schema':
      return 'JSON enforced'
    case 'json_object':
      return 'JSON only, shape not enforced'
    case 'prompt':
      return 'Best effort, nothing enforced'
    default:
      return 'Not measured'
  }
}

/** The wire form addressing ONE custom endpoint - a Lab variant, never a stored setting. */
export function customVariantId(id: string): string {
  return `custom:${id}`
}

/**
 * One selectable model in a provider's curated list.
 *
 * The list is a CONVENIENCE, never a constraint: nothing in the pipeline validates the
 * string (it is passed verbatim into the CLI's argv - see `src/llm/claude.rs` and friends),
 * so the picker always allows a free-text value for a model we haven't listed.
 */
export interface LlmModelOption {
  /** The exact string handed to the backend - a CLI alias or a full model id. */
  id: string
  /** What the picker shows. */
  label: string
  /** Optional one-liner shown under the label. */
  note?: string
}

export interface LlmProviderMeta {
  id: LlmProviderId
  name: string
  /** One-line "what this is", shown on the card. */
  blurb: string
  /**
   * 'cli' → shells out to an installed binary; 'custom' → a user-configured cloud endpoint
   * (an HTTP call on their own API key).
   */
  kind: 'cli' | 'custom'
  /** The executable we look for. null for a custom endpoint (it is an HTTP call). */
  bin: string | null
  /** Shown when the CLI is not installed - the command that installs it. */
  installHint?: string
  installUrl?: string
  /**
   * Whether this backend actually passes a model through. FALSE for copilot, whose argv is
   * built without a model flag and never reads `cfg.model` (`src/llm/copilot.rs`) - the
   * picker must show "managed by the CLI" there rather than a control that silently does
   * nothing. Mirrors the doc comment on `llm_provider_model` in meridian-core/src/settings.rs,
   * which likewise omits copilot.
   */
  supportsModelOverride: boolean
  /**
   * The curated model list offered in the picker. May be empty for a provider whose model
   * names we can't enumerate confidently - the picker then falls back to free text alone.
   * These are CLI subprocesses with no models endpoint, so this list is hand-maintained;
   * only custom endpoints can be enumerated live (see `list_custom_llm_provider_models`).
   */
  models: LlmModelOption[]
  /**
   * Where the user opts their prompts OUT of model training. This is ACCOUNT-LEVEL for
   * every provider (a subprocess can't flip it - Meridian only disables telemetry per
   * call), so the UI links it rather than pretending to toggle it.
   */
  privacyUrl: string | null
  /** Human label for the account setting the privacyUrl leads to. */
  privacyLabel?: string
}

// Order IS the recommendation: coding agents first (frontier-model accuracy). The picker
// renders them in this order.
export const LLM_PROVIDERS: LlmProviderMeta[] = [
  {
    id: 'claude',
    name: 'Claude Code',
    blurb: 'Uses your Claude subscription through the claude CLI.',
    kind: 'cli',
    bin: 'claude',
    installHint: 'npm i -g @anthropic-ai/claude-code',
    installUrl: 'https://claude.com/claude-code',
    privacyUrl: 'https://claude.ai/settings/data-privacy-controls',
    privacyLabel: 'Claude - Privacy controls',
    supportsModelOverride: true,
    // The claude CLI accepts both tier aliases and full model ids. Aliases are listed
    // first: they keep working across releases, where a pinned id eventually retires.
    models: [
      { id: 'opus', label: 'Opus', note: 'Most capable - follows each new Opus release' },
      { id: 'sonnet', label: 'Sonnet', note: 'Balanced speed and quality' },
      { id: 'haiku', label: 'Haiku', note: 'Fastest and cheapest' },
      { id: 'claude-opus-4-8', label: 'Claude Opus 4.8', note: 'Stays on this exact model' },
      { id: 'claude-sonnet-5', label: 'Claude Sonnet 5', note: 'Stays on this exact model' },
      { id: 'claude-haiku-4-5', label: 'Claude Haiku 4.5', note: 'Stays on this exact model' },
    ],
  },
  {
    id: 'codex',
    name: 'Codex',
    blurb: 'Uses your ChatGPT subscription through the codex CLI.',
    kind: 'cli',
    bin: 'codex',
    installHint: 'npm i -g @openai/codex',
    installUrl: 'https://developers.openai.com/codex/cli',
    privacyUrl: 'https://chatgpt.com/#settings/DataControls',
    privacyLabel: 'ChatGPT - Data Controls',
    supportsModelOverride: true,
    models: [
      { id: 'gpt-5.5', label: 'GPT-5.5', note: 'Latest general model' },
      { id: 'gpt-5.1-codex', label: 'GPT-5.1 Codex', note: 'Tuned for coding' },
      { id: 'gpt-5.1', label: 'GPT-5.1' },
    ],
  },
  {
    id: 'cursor',
    name: 'Cursor',
    blurb: 'Uses your Cursor subscription through the cursor-agent CLI.',
    kind: 'cli',
    bin: 'cursor-agent',
    installHint: 'curl https://cursor.com/install -fsS | bash',
    installUrl: 'https://cursor.com/cli',
    privacyUrl: 'https://cursor.com/settings',
    privacyLabel: 'Cursor - Privacy Mode',
    supportsModelOverride: true,
    // Deliberately empty: cursor-agent takes --model, but its accepted values aren't
    // pinned anywhere we can verify, and an invented id would be passed verbatim into
    // argv and fail at run time. Free text only until we can source a real list.
    models: [],
  },
  {
    id: 'copilot',
    name: 'GitHub Copilot',
    blurb: 'Uses your Copilot subscription through the copilot CLI.',
    kind: 'cli',
    bin: 'copilot',
    installHint: 'npm i -g @github/copilot',
    installUrl: 'https://github.com/features/copilot/cli',
    privacyUrl: 'https://github.com/settings/copilot/features',
    privacyLabel: 'GitHub - Copilot policies',
    // The copilot backend builds its argv without a model flag and never reads cfg.model
    // (src/llm/copilot.rs) - the model is whatever the CLI itself is configured to use.
    // Offering a picker here would write a setting the backend drops on the floor.
    supportsModelOverride: false,
    models: [],
  },
]

/**
 * Look up one provider's metadata, or undefined for an unknown id.
 *
 * `LLM_PROVIDERS` holds only the built-ins that get a card in the grid, so 'custom' has to
 * be answered from [`CUSTOM_PROVIDER_META`] - exactly as `llmProvider` does. Without that,
 * `supportsModelOverride('custom')` would answer false even though a custom endpoint does
 * carry a model, which is the wrong answer for any caller that asks by id.
 */
export function providerMeta(id: LlmProviderId): LlmProviderMeta | undefined {
  if (id === 'custom') return CUSTOM_PROVIDER_META
  return LLM_PROVIDERS.find((p) => p.id === id)
}

/**
 * The curated models for a provider - empty when we can't enumerate them (cursor) or the
 * backend ignores the model entirely (copilot). Callers must treat empty as "free text
 * only", NOT as "this provider has no models".
 */
export function modelsFor(id: LlmProviderId): LlmModelOption[] {
  return providerMeta(id)?.models ?? []
}

/**
 * Whether a model control should be offered for this provider at all. False means the
 * chosen model would be silently discarded by the backend - show an explanatory note
 * instead of a picker.
 */
export function supportsModelOverride(id: LlmProviderId): boolean {
  return providerMeta(id)?.supportsModelOverride ?? false
}

/**
 * The stored default - Claude, the first/recommended coding agent (matches
 * `LlmProvider::default()` in meridian-core/src/llm_provider.rs). An unknown or
 * unconfigured choice resolves here.
 */
export const DEFAULT_LLM_PROVIDER: LlmProviderId = 'claude'

/**
 * The rough share of a plan's usage limit that Meridian's hourly summaries consume - one
 * short request per active hour. Surfaced to reassure users that pointing Meridian at
 * their coding-agent subscription won't eat into their own coding headroom.
 */
export const USAGE_FOOTPRINT_NOTE =
  'Meridian sends about one short request per active hour - well under 1% of your plan’s usage limits, so it won’t eat into your own coding.'

/**
 * The warning shown wherever a custom endpoint is added or listed.
 *
 * Every other provider is flat-rate: a CLI spends a subscription the user already pays
 * for. A custom endpoint is the ONLY one that bills per
 * call, on the user's own key, with no cap Meridian can enforce - the pipeline runs
 * unattended every hour, and testing an endpoint alone spends up to one request per schema.
 * So the guidance is free-tier only, stated where the key is typed rather than buried in
 * docs nobody reads before pasting a billing-enabled key.
 */
export const CUSTOM_PROVIDER_COST_NOTE =
  'Use a free-tier API key only. A custom endpoint bills your own account for every call and Meridian cannot cap what it spends - the pipeline runs each hour on its own, and testing one costs a few requests.'

/**
 * The OpenAI-compatible endpoints offered as presets, so the common case is a name and a
 * key rather than a URL nobody can be expected to remember.
 *
 * `baseUrl` is the OpenAI-compatible root - Meridian appends `/chat/completions`. These are
 * conveniences ONLY: nothing about a preset grants capability. What an endpoint can
 * actually do is measured when it is added (see `SchemaRung` / `llm::probe`), because
 * "OpenAI-compatible" is not one contract - measured, OpenAI rejects a schema Gemini
 * accepts, and Gemini refuses strict mode for one of the four schemas it otherwise handles.
 *
 * Gemini leads because it is the one endpoint measured end-to-end against the real
 * pipeline schemas, and it has a free tier - which is the only kind we want here (see
 * `CUSTOM_PROVIDER_COST_NOTE`).
 */
export interface CustomVendorPreset {
  id: string
  name: string
  /** Empty = the user types their own (the `other` escape hatch). */
  baseUrl: string
  /** Where to get a key. */
  keyUrl?: string
  /** Shown under the picker - free tier or not is the deciding fact here. */
  hint?: string
}

export const CUSTOM_VENDOR_PRESETS: CustomVendorPreset[] = [
  {
    id: 'gemini',
    name: 'Google Gemini',
    baseUrl: 'https://generativelanguage.googleapis.com/v1beta/openai',
    keyUrl: 'https://aistudio.google.com/apikey',
    hint: 'Has a free tier. Verified against Meridian’s schemas.',
  },
  {
    id: 'groq',
    name: 'Groq',
    baseUrl: 'https://api.groq.com/openai/v1',
    keyUrl: 'https://console.groq.com/keys',
    hint: 'Has a free tier.',
  },
  {
    id: 'openrouter',
    name: 'OpenRouter',
    baseUrl: 'https://openrouter.ai/api/v1',
    keyUrl: 'https://openrouter.ai/keys',
    hint: 'Free models exist (ids ending in :free). Paid models bill per call.',
  },
  {
    id: 'openai',
    name: 'OpenAI',
    baseUrl: 'https://api.openai.com/v1',
    keyUrl: 'https://platform.openai.com/api-keys',
    hint: 'Paid only - every call is billed.',
  },
  {
    id: 'other',
    name: 'Other (OpenAI-compatible)',
    baseUrl: '',
    hint: 'Any endpoint serving /chat/completions.',
  },
]

export function customVendorPreset(id: string): CustomVendorPreset | undefined {
  return CUSTOM_VENDOR_PRESETS.find(v => v.id === id)
}

/**
 * The stand-in for `custom` in any lookup - NOT a card in `LLM_PROVIDERS`.
 *
 * `custom` is a KIND, not an instance: the user may have several endpoints configured, so
 * there is no single card for it (exactly why `LlmProvider::builtins()` exists in Rust).
 * But `llmProvider('custom')` is still asked for a name by everything that renders the
 * CHOSEN provider, so the lookup answers honestly and the grid still renders only the
 * built-ins.
 *
 * Surfaces holding the registry should show the endpoint's own name instead of this one.
 */
export const CUSTOM_PROVIDER_META: LlmProviderMeta = {
  id: 'custom',
  name: 'Custom endpoint',
  blurb: 'Your own OpenAI-compatible cloud endpoint, on your own API key.',
  kind: 'custom',
  bin: null,
  // Account-level, and it varies per vendor - there is no one link to send them to.
  privacyUrl: null,
  // A custom endpoint carries its model per-row (`CustomEndpoint.model`, sent as the
  // "model" body field in src/llm/openai_compat.rs) rather than through the shared
  // `llm_provider_model` setting - but a model IS selectable, hence true.
  supportsModelOverride: true,
  // Empty on purpose: these are the one provider whose models can be enumerated LIVE,
  // from the endpoint's own {base_url}/models. Nothing to hand-maintain here.
  models: [],
}

export function llmProvider(id: LlmProviderId): LlmProviderMeta {
  if (id === 'custom') return CUSTOM_PROVIDER_META
  // An unknown id resolves to the default (Claude), matching the Rust resolver.
  return LLM_PROVIDERS.find(p => p.id === id)
    ?? LLM_PROVIDERS.find(p => p.id === DEFAULT_LLM_PROVIDER)
    ?? LLM_PROVIDERS[0]
}
