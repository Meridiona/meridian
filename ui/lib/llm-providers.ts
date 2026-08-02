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
    // Pinned to the cursor-agent build Meridian is verified against - mirrors
    // CURSOR_INSTALL_CMD in meridian-core/src/llm_provider.rs (the authoritative copy the
    // tray actually runs). cursor.com/install is a rolling script with no version flag,
    // so the sed rewrites its version strings; matching by pattern keeps working after
    // Cursor bumps their script.
    installHint:
      "curl -fsSL https://cursor.com/install | sed -E 's#[0-9]{4}\\.[0-9]{2}\\.[0-9]{2}-[0-9a-f]{7}#2026.07.16-899851b#g' | bash",
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
 * The providers shown at the TOP LEVEL of the picker, in order — the three coding-agent CLIs
 * we recommend. Copilot is still a valid stored value and is still probed by the backend, but
 * it is deliberately NOT offered here: it needs org policy that often blocks it and it can't
 * take a model, so it made a poor default. A user who already has it selected is handled with
 * a small banner rather than a card (see <LlmProviderPicker>).
 */
export const CHOOSER_PROVIDER_IDS: LlmProviderId[] = ['claude', 'codex', 'cursor']

/** The single shared heading for the provider chooser - same words in the wizard and Settings
 *  so the one choice reads identically wherever it is made. Plain hyphens (user-facing). */
export const LLM_INTRO_TITLE = 'Choose your AI provider'

/** The shared sub-heading. States the job and the escape hatch, and stops.
 *
 *  It used to also quote the usage footprint ("well under 1% of your plan") - a THIRD wording
 *  of a number that `LLM_RECOMMENDED_NOTE` states directly under the grid, in the same
 *  viewport, as 2%. Three sizes of the same claim, two of them disagreeing, is how a
 *  reassurance turns into a reason for doubt. The number is made once now, below the tiles. */
export const LLM_INTRO_BODY =
  'Meridian uses this to write your hourly summaries and worklogs. Pick a coding agent you already pay for, or bring your own API key.'

/**
 * The one question asked before any provider is shown, and the two answers.
 *
 * It exists because the old chooser put four tiles in front of someone who had, a moment
 * earlier, only asked to draft a task. Three of those tiles are useless without a paid
 * subscription and the fourth was labelled "advanced" - so the screen silently required the
 * user to know which group they were in before it would help them. Asking outright costs one
 * click and lets each answer get a screen built for it.
 *
 * THE RECOMMENDATION IS A BADGE, NOT A PARAGRAPH. Both answers are legitimate and one of
 * them is free; ranking them in prose would read as pressure to spend money, and burying the
 * ranking entirely would leave someone with a Claude subscription on the weaker path for no
 * reason. A badge states it and gets out of the way.
 */
/** The gate's recommendation. Used ONLY where it ranks one thing against another - which,
 *  on the gate, it does: a paid coding agent is the more accurate path than the free one.
 *
 *  It is deliberately NOT the chooser grid's badge any more. All three tiles there carried
 *  it, and a badge every option wears cannot rank anything: three cards each shouting
 *  RECOMMENDED just made the row loud. The grid uses the kind labels below instead, which
 *  say what the tile IS - the real distinction between the three and the fourth. */
export const LLM_RECOMMENDED_BADGE = 'RECOMMENDED'

/** How the two paths rank, as the pair of badges BOTH screens show.
 *
 *  The gate asks which path you are on; the chooser grid then shows one path's tiles beside
 *  the other's. Those are the same claim made twice, so they read from one record - a user
 *  who is told BEST ACCURACY on the question and then sees nothing on the tiles has been
 *  given a ranking and then had it taken away at the moment they act on it.
 *
 *  `badge` is the ranking itself and renders FILLED; `note` is what you get and renders
 *  outlined. Neither is coloured - see LlmProviderGate's header for why. */
export interface LlmRank { badge: string; note: string }
export const LLM_RANK_SUBSCRIPTION: LlmRank = {
  badge: LLM_RECOMMENDED_BADGE,
  note: 'BEST ACCURACY',
}
export const LLM_RANK_FREE: LlmRank = { badge: 'FREE', note: 'GOOD ACCURACY' }

export const LLM_GATE_TITLE = 'Do you have a Claude, ChatGPT or Cursor subscription?'
export const LLM_GATE_BODY =
  'Meridian writes your summaries with an AI engine. It can use a subscription you already pay for, or set you up with a free one.'

export interface LlmGateChoice {
  value: 'subscription' | 'free'
  label: string
  /** The primary badge - the recommendation itself. */
  badge: string
  /** The second badge - what you get, in two words. */
  note: string
  /** One line naming what happens next, so neither answer is a leap of faith. */
  detail: string
}

export const LLM_GATE_CHOICES: LlmGateChoice[] = [
  {
    value: 'subscription',
    ...LLM_RANK_SUBSCRIPTION,
    label: 'Yes, I have one',
    detail: 'Claude Code, Codex or Cursor - Meridian runs on the plan you already have.',
  },
  {
    value: 'free',
    ...LLM_RANK_FREE,
    label: 'No, set me up',
    detail: 'A free Groq key, in three steps. No card, no subscription.',
  },
]


/** The one line under the recommended row.
 *
 *  It used to be a 20-word sentence about frontier models and marginal cost, set at 11px -
 *  which is the size you use for a footnote nobody is expected to read, carrying the one
 *  fact that decides the question. The real objection to pointing Meridian at a plan you
 *  already pay for is "will it eat my usage", so that is the whole line now, short enough
 *  to be set at a readable size. */
export const LLM_RECOMMENDED_NOTE = 'Meridian uses less than 2% of your daily usage.'

/**
 * The rough share of a plan's usage limit that Meridian's hourly summaries consume - one
 * short request per active hour. Surfaced to reassure users that pointing Meridian at
 * their coding-agent subscription won't eat into their own coding headroom.
 *
 * It quotes the SAME figure as `LLM_RECOMMENDED_NOTE` on purpose. The two used to disagree
 * ("less than 2%" on the grid, "well under 1%" one click later on the detail screen), which
 * is the kind of thing a user reads as a number chosen to sound good rather than measured.
 * The higher, more conservative one is the one both say.
 */
export const USAGE_FOOTPRINT_NOTE =
  'About one short request per active hour - less than 2% of your daily usage, so it will not eat into your own coding.'

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

/**
 * GROQ IS THE ONLY PRESET, and that is a product decision rather than a shortlist.
 *
 * This list used to carry Gemini, OpenRouter, OpenAI and a free-text "other". Every one of
 * them made the no-subscription path a configuration exercise: pick a vendor, judge whether
 * its tier is free, find its key page, then choose a model from a list nobody outside the
 * team could rank. That is the WRONG question to put in front of someone whose answer to
 * "do you have a coding-agent subscription" was no - they are here precisely because they
 * do not want to make this decision.
 *
 * So there is exactly one: free, no card, no prompt retention, no training on your data
 * (see [`GROQ`]). One vendor means the whole screen can be a three-step walkthrough with a
 * single paste field instead of a form - which is what `<GroqSetup>` renders.
 *
 * The cost of dropping "other" is real and worth naming: an advanced user can no longer
 * point Meridian at an arbitrary OpenAI-compatible endpoint from the UI. The registry
 * underneath is unchanged and still holds any endpoint added by an older build, so nobody's
 * existing configuration breaks - only the ADD path narrowed.
 */
export const CUSTOM_VENDOR_PRESETS: CustomVendorPreset[] = [
  {
    id: 'groq',
    name: 'Groq',
    baseUrl: 'https://api.groq.com/openai/v1',
    keyUrl: 'https://console.groq.com/keys',
    hint: 'Free. No card required.',
  },
]

/**
 * Everything the no-subscription path says and points at, in one record.
 *
 * The privacy claims are LINKED, not just asserted. This screen asks someone to paste a key
 * on the strength of a sentence about data handling, and a sentence with no source is
 * exactly the thing a careful user should not accept - so each claim carries the vendor's
 * own page next to it, and the wording stays close enough to that page to survive being
 * checked against it.
 *
 * All hyphens plain, per the user-facing text rule.
 */
export const GROQ = {
  vendor: 'groq',
  /** The name stored on the registry row, and what the rest of the app then calls it. */
  name: 'Groq',
  baseUrl: 'https://api.groq.com/openai/v1',
  signUpUrl: 'https://console.groq.com/login',
  keyUrl: 'https://console.groq.com/keys',
  privacyUrl: 'https://groq.com/privacy-policy/',
  termsUrl: 'https://groq.com/terms-of-sale/',
  /** Shown at the very top - the single fact that makes this option make sense. */
  freeBadge: 'FREE',
  headline: 'Groq Cloud',
  blurb: 'A free API key, and Meridian handles the rest. No card, no subscription.',
  /** The three claims, each with the page that backs it. */
  trust: [
    'Groq does not train any model on what you send.',
    'Prompts and replies are not retained after the request is answered.',
    'Only the hour Meridian is summarising is ever sent - never your files or your screen.',
  ],
  /** Keys look like `gsk_…` - shown as a placeholder so a wrong paste is obvious. */
  keyPlaceholder: 'gsk_…',
} as const

/**
 * Which Groq model Meridian picks, best first - matched as a PREFIX against whatever the
 * endpoint's own `/models` actually returns.
 *
 * Asking the endpoint rather than hardcoding one id is the whole point. Groq's catalogue
 * turns over quickly, and a hardcoded id that has been retired fails at the first real hour
 * rather than at setup - the worst possible place. Prefixes (not exact ids) so a dated
 * revision like `…-0905` still matches its family.
 *
 * Order is by structured-output support, which is the only property that matters here: the
 * pipeline asks for JSON conforming to a schema, and an endpoint that cannot do that is
 * rejected by `production_eligible` no matter how good its prose is.
 */
export const GROQ_MODEL_PREFERENCE: string[] = [
  'moonshotai/kimi-k2',
  'openai/gpt-oss-120b',
  'openai/gpt-oss-20b',
  'llama-3.3-70b',
  'meta-llama/llama-4-maverick',
  'meta-llama/llama-4-scout',
]

/** Model families that cannot answer a chat completion at all - speech, embeddings and the
 *  safety classifiers Groq lists alongside the chat models. Excluded before ranking so a
 *  fallback can never land on one. */
const GROQ_NON_CHAT = ['whisper', 'tts', 'embed', 'guard', 'prompt-guard']

/**
 * Choose the model to configure from the ids the endpoint reported.
 *
 * Falls back to the first usable id when nothing preferred is on offer, and to `null` only
 * when the list is empty or entirely non-chat - which the caller must treat as "could not
 * set this up", never as "use the default", since there is no default to use.
 */
export function pickGroqModel(available: string[]): string | null {
  const usable = available.filter(
    (m) => !GROQ_NON_CHAT.some((bad) => m.toLowerCase().includes(bad)),
  )
  for (const want of GROQ_MODEL_PREFERENCE) {
    const hit = usable.find((m) => m.toLowerCase().startsWith(want.toLowerCase()))
    if (hit) return hit
  }
  return usable[0] ?? null
}

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

/**
 * The EXACT settings fields to write when the user picks a provider - the single source of
 * truth for both surfaces that can make that choice (the setup wizard and Settings →
 * Intelligence).
 *
 * Shared because the two hand-rolled copies drifted, and the drift was silent: Settings
 * cleared `llm_provider_model` and setup did not, so switching provider in the WIZARD carried
 * a stale model override onto the new CLI. `src/llm/config.rs` still passes that value into
 * `--model`, so e.g. a leftover `"opus"` became `codex --model opus` - a failure surfacing an
 * hour later, far from the click. On Cursor it is worse than a bad arg: a non-empty override
 * suppresses the pinned ZDR-eligible default in `src/llm/cursor.rs`, so a stale value silently
 * moves the user off the model we pinned partly for that guarantee.
 *
 * `custom` is a KIND, not an endpoint, so it always travels with the id of the chosen one
 * (which `update_settings` requires and validates) - and it keeps its model, because a custom
 * endpoint's model is a real per-row setting rather than a leftover.
 */
export function providerChoiceFields(
  id: LlmProviderId,
  customId?: string,
): { llm_provider: LlmProviderId; llm_provider_custom_id?: string | null; llm_provider_model?: null } {
  return id === 'custom'
    ? { llm_provider: id, llm_provider_custom_id: customId ?? null }
    : { llm_provider: id, llm_provider_model: null }
}

export function llmProvider(id: LlmProviderId): LlmProviderMeta {
  if (id === 'custom') return CUSTOM_PROVIDER_META
  // An unknown id resolves to the default (Claude), matching the Rust resolver.
  return LLM_PROVIDERS.find(p => p.id === id)
    ?? LLM_PROVIDERS.find(p => p.id === DEFAULT_LLM_PROVIDER)
    ?? LLM_PROVIDERS[0]
}

/**
 * In-app sign-in descriptor for the providers whose CLI authenticates against
 * the user's OWN subscription via a browser OAuth that Meridian can drive.
 *
 * Single source of truth for BOTH halves of that flow, which previously lived in
 * two files and could desync silently:
 *   - `LlmProviderDetail.tsx` renders `label`/`account`/`subscription`/`cmd`.
 *   - `LlmProviderPicker.tsx` invokes `trayCommand`.
 *
 * The hazard was that the tray command name appeared once as a literal here and
 * once in the picker's own map, so adding a provider to only one place gave it a
 * "Sign in" button wired to nothing - or, worse, left an entry pointing at
 * another vendor's login. Keeping them in one record makes that impossible:
 * a provider either has an entry (button + command) or it does not.
 *
 * Absent id = no in-app sign-in (Copilot, custom cloud endpoints), which is a
 * valid state, not an omission.
 */
export interface LlmSignIn {
  /** Button label. */
  label: string
  /** Account brand named in the copy, e.g. "ChatGPT" for Codex. */
  account: string
  /** Subscription named in the copy. */
  subscription: string
  /** Terminal fallback shown when the in-app flow cannot be used. */
  cmd: string
  /** `#[tauri::command]` the picker invokes (see `tray/src-tauri/src/commands/setup.rs`). */
  trayCommand: string
}

export const LLM_SIGN_IN: Record<string, LlmSignIn> = {
  cursor: {
    label: 'Sign in to Cursor',
    account: 'Cursor',
    subscription: 'Cursor subscription',
    cmd: 'cursor-agent login',
    trayCommand: 'cursor_sign_in',
  },
  codex: {
    label: 'Sign in to Codex',
    account: 'ChatGPT',
    subscription: 'ChatGPT subscription',
    cmd: 'codex login',
    trayCommand: 'codex_sign_in',
  },
  claude: {
    label: 'Sign in to Claude',
    account: 'Claude',
    subscription: 'Claude subscription',
    cmd: 'claude auth login',
    trayCommand: 'claude_sign_in',
  },
}

/** The sign-in descriptor for `id`, or `null` when it has no in-app flow. */
export function llmSignIn(id: string): LlmSignIn | null {
  return LLM_SIGN_IN[id] ?? null
}
