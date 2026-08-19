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

/**
 * The key addressing ONE custom endpoint specifically - `custom:<id>`, matching Rust's
 * `provider_key`/`rate_limit::custom_key` exactly (never re-derive this format locally, or
 * the two will drift the way the health-cache lookup once did).
 *
 * Two uses: an LLM-Lab variant id (never a stored setting), and the `status` map's key for
 * the ACTIVE custom endpoint's own test/health row (`LlmProviderPicker`'s `CloudPresetTile`) -
 * the bare `'custom'` id is ambiguous the moment more than one endpoint is configured, since
 * every custom row shares the `LlmProvider::Custom` wire form.
 */
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
    // Vendor-agnostic on purpose: which free key (Groq or Ollama) is a choice made on the
    // NEXT screen, not something to name here and then contradict a click later.
    detail: 'A free API key, in a few steps. No card, no subscription.',
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
 * THERE IS NO ADD-AN-ENDPOINT PATH ANY MORE, and the vendor-preset machinery that served it
 * is gone with it (`CUSTOM_VENDOR_PRESETS`, `customVendorPreset`, `CustomVendorPreset`,
 * `CUSTOM_PROVIDER_COST_NOTE`, and the `<AddForm>`/`<AddCustomProvider>` pair).
 *
 * The list had already collapsed to Groq alone, which made the form a questionnaire with
 * one possible answer: pick the only vendor, keep the URL it filled in, press "List models"
 * to choose from ids nobody outside the team can rank, then guess two rate-limit numbers the
 * verdict would otherwise nag about. `<CloudKeySetup>` answers every one of those for the
 * user from one pasted key, so the form was a second, worse route to the same endpoint - and
 * the "+ Add a custom endpoint" tile advertised it on the settings screen as though it were a
 * capability rather than a leftover.
 *
 * The cost, stated plainly: an arbitrary OpenAI-compatible endpoint can no longer be added
 * from the UI - only the two curated presets below can. The registry underneath is
 * untouched - `add_custom_llm_provider` still exists, `<CloudKeySetup>` still calls it, and
 * an endpoint added by an older build (or a third vendor added by hand) still loads, runs
 * and can be selected. Only the way to CREATE one from the UI narrowed to these presets.
 */

/**
 * Everything a no-subscription preset says and points at, in one record - the shape both
 * `GROQ` and `OLLAMA` satisfy, and `<CloudKeySetup>` is built entirely against.
 *
 * The privacy claims are LINKED, not just asserted. The setup screen asks someone to paste a
 * key on the strength of a sentence about data handling, and a sentence with no source is
 * exactly the thing a careful user should not accept - so each claim carries the vendor's
 * own page next to it, and the wording stays close enough to that page to survive being
 * checked against it.
 *
 * All hyphens plain, per the user-facing text rule.
 */
export interface CloudKeyPreset {
  /** Stored on the registry row as `CustomLlmProvider::vendor` - display/provenance only in
   *  Rust, but read here (and by `CustomVendorLogo`) to pick the right setup screen and logo. */
  vendor: string
  /** The name stored on the registry row, and what the rest of the app then calls it. */
  name: string
  baseUrl: string
  /** Where a free key is created. Doubles as the sign-up path on every vendor here - signed
   *  out, each one asks you to create an account and then lands on the key screen. MUST be
   *  the link that survives sign-up/sign-in and still lands on the actual key screen - a
   *  vendor whose auth redirect drops the original destination needs the return path spelt
   *  out in the URL itself (see `OLLAMA.keyUrl`), not just linked to the page's plain address. */
  keyUrl: string
  /** Step 1's instructions - what to do after `keyUrl` opens. Per-preset because the vendors
   *  genuinely differ here: Groq's link lands directly on a dedicated keys page with nothing
   *  else on it, while Ollama's settings are a tabbed page, so the click that Groq's copy
   *  doesn't need to mention (find, then press, "Create API key") is one Ollama's copy has to
   *  spell out - a generic instruction that fit one vendor silently stopped fitting the other. */
  keyStepBody: string
  privacyUrl: string
  termsUrl: string
  /** Shown at the very top - the single fact that makes this option make sense. */
  freeBadge: string
  /** The second badge, next to FREE - what happens to what gets sent, ranked with the price
   *  because that answer used to live three paragraphs down in a block most people skip. */
  privacyBadge: string
  headline: string
  blurb: string
  /** The claims, each backed by the page in `privacyUrl`/`termsUrl`. Ordered by what a
   *  careful reader asks first: is it kept, is it learned from, how much of my machine goes
   *  with it. */
  trust: string[]
  /** Shown as the key field's placeholder, so a wrong paste is obvious. */
  keyPlaceholder: string
  /** Requests-per-minute ceiling this preset's free tier is KNOWN to allow, or `0` when the
   *  quota isn't measured in requests at all (see `meridian_core::llm_capacity::assess` -
   *  `0` means "not known", never "unmetered"). Never guessed. */
  freeRpm: number
  /** Requests-per-day ceiling, same "0 = not known" convention as `freeRpm`. */
  freeRpd: number
  /** Which model Meridian picks, best first - matched as a PREFIX against whatever the
   *  endpoint's own `/models` actually returns. See `pickCloudModel`. */
  modelPreference: string[]
  /** Model-name fragments that can't answer a chat completion at all (speech, embeddings,
   *  safety classifiers) - excluded before ranking so a fallback can never land on one. */
  nonChatFilter: string[]
}

/**
 * Groq's own free-tier limits (30 RPM, 1,000 RPD - console.groq.com/docs/rate-limits, checked
 * Aug 2026) for the gpt-oss pair this preset pins.
 *
 * These used to be written as 0/0 - "unknown rather than guessed" - which was right while the
 * model was whatever the key happened to serve, but is no longer: the model is pinned to
 * gpt-oss (see `modelPreference` below) and both members carry these exact numbers. The cost
 * of leaving them unknown was a permanent notice on the endpoint card asking the user to go
 * and look up a limit we already know, and unpaced requests on a key that does have a
 * per-minute cap.
 *
 * Understating a PAID key is the deliberate direction to be wrong in: a working day needs 23
 * requests against 1,000, so the verdict reads "sufficient" either way, and pacing at 30/min
 * costs a paid user nothing at this volume.
 */
export const GROQ: CloudKeyPreset = {
  vendor: 'groq',
  name: 'Groq',
  baseUrl: 'https://api.groq.com/openai/v1',
  keyUrl: 'https://console.groq.com/keys',
  // This link IS the keys page, and Groq's own sign-up redirect returns here afterward - so
  // the whole trip is "sign up if you haven't, then you're already looking at Create API key".
  keyStepBody:
    'Sign up with Google or an email address if you have not already - no card is asked for. ' +
    'Then press Create API key and copy it, it is shown once.',
  privacyUrl: 'https://groq.com/privacy-policy/',
  // Terms of USE, not the old /terms-of-sale/ - that path 404s, and a dead link under a
  // privacy claim is worse than no link: it reads as a claim that cannot be checked.
  termsUrl: 'https://groq.com/terms-and-conditions/',
  freeBadge: 'FREE',
  privacyBadge: 'ZERO DATA RETENTION',
  headline: 'Groq Cloud',
  blurb: 'A free API key, and Meridian handles the rest. No card, no subscription.',
  trust: [
    'Your prompts and replies are not logged or stored - Groq answers the request and drops it.',
    'Nothing you send is used to train any model.',
    'Meridian sends one hour of activity at a time. Your screen, your files and your keystrokes stay on this Mac.',
  ],
  // Keys look like `gsk_…`.
  keyPlaceholder: 'gsk_…',
  freeRpm: 30,
  freeRpd: 1000,
  // WHY THE LIST IS ONLY TWO ENTRIES (checked against Groq's docs, Aug 2026). On Groq,
  // `response_format: json_schema` is supported by `openai/gpt-oss-120b` and
  // `openai/gpt-oss-20b` and NOTHING ELSE - every other model tops out at JSON Object mode,
  // which produces valid JSON of no particular shape. That is exactly the failure this
  // pipeline cannot absorb: a reply that parses but omits a field does not error, it drops
  // an hour silently. So a bigger or better-reasoning model that lacks strict schema support
  // is not a trade-off here, it is disqualified - which is why `llama-3.3-70b`,
  // `moonshotai/kimi-k2` and the `llama-4-*` pair were removed. Ranking them ahead of the
  // gpt-oss pair meant the setup screen picked a model the probe would then refuse, turning
  // a working key into "could not return the structured replies Meridian needs".
  //
  // ACCURACY AND QUOTA DO NOT TRADE OFF AGAINST EACH OTHER HERE. Both survivors carry the
  // same free-tier limits (30 RPM, 1,000 RPD, 8K TPM, 200K TPD), so taking the larger, more
  // accurate 120b gives up no headroom whatsoever. 20b stays as the fallback for a key that
  // only serves it, and is roughly twice as fast if that ever matters more.
  modelPreference: ['openai/gpt-oss-120b', 'openai/gpt-oss-20b'],
  nonChatFilter: ['whisper', 'tts', 'embed', 'guard', 'prompt-guard'],
}

/**
 * Ollama Cloud (`ollama.com`) - a second no-subscription preset, added alongside Groq rather
 * than instead of it because the two fail in DIFFERENT ways: Groq's free tier caps every
 * plain chat model's TPM well below what an hour of activity needs, while Ollama's quota is
 * GPU-time/session-metered instead (session limit resets every ~5h, weekly every ~7 days) -
 * a genuinely different shape of "free", not just a second key to the same problem.
 *
 * Verified live (this session): `GET https://ollama.com/v1/models` and
 * `POST https://ollama.com/v1/chat/completions` are real, OpenAI-compatible, and return the
 * standard nested error envelope on a bad key - the same wire protocol Groq already uses
 * through `OpenAiCompatBackend`, so this preset needs no new Rust.
 */
export const OLLAMA: CloudKeyPreset = {
  vendor: 'ollama',
  name: 'Ollama',
  baseUrl: 'https://ollama.com/v1',
  // NOT the bare `/settings/keys` address - verified live (this session): visiting that
  // signed-out redirects to `/signin` with no return path attached at all, so a user who
  // just created an account lands on ollama.com's general homepage with no obvious way
  // back to the key screen. `/signin?next=...` carries the destination through their
  // WorkOS auth redirect instead (confirmed via the `state` param, which base64-decodes to
  // exactly `/settings/keys`), so sign-up and sign-in both land back here afterward - the
  // same round-trip Groq's own keys link gives for free.
  keyUrl: 'https://ollama.com/signin?next=%2Fsettings%2Fkeys',
  // Spelt out, unlike Groq's: Groq's link opens a page with nothing else on it, but Ollama's
  // settings are a tabbed page, so "create an API key" alone leaves the actual click
  // unnamed. Named explicitly so nobody has to go hunting for it.
  keyStepBody:
    'Sign up with Google or an email address if you have not already - no card is asked for. ' +
    'You will land on the Keys section of Settings - press Create API key there and copy it, ' +
    'it is shown once.',
  privacyUrl: 'https://ollama.com/privacy',
  termsUrl: 'https://ollama.com/terms',
  freeBadge: 'FREE',
  // Not "ZERO DATA RETENTION" like Groq's badge - Ollama's own privacy policy (fetched
  // during this feature's research) says cloud prompts/responses are processed
  // TRANSIENTLY and never used for training, which is the same substance stated in the
  // vendor's own words rather than Groq's phrasing borrowed for a different policy.
  privacyBadge: 'TRANSIENT PROCESSING',
  headline: 'Ollama Cloud',
  blurb: 'A free API key, and Meridian handles the rest. No card, no subscription.',
  trust: [
    'Cloud prompts and responses are processed transiently, not retained - see ollama.com/privacy.',
    'Nothing you send is used to train any model.',
    'Meridian sends one hour of activity at a time. Your screen, your files and your keystrokes stay on this Mac.',
  ],
  // Ollama API keys have no single documented prefix the way Groq's `gsk_…` does.
  keyPlaceholder: 'Paste your Ollama API key',
  // Ollama publishes NO machine-readable rate limits - its free tier is GPU-time/session
  // metered (session limit every ~5h, weekly every ~7 days), not a request count, and its
  // API returns no `x-ratelimit-*` headers to learn one from reactively either. `0/0` here
  // is the honest "not known" (see `meridian_core::llm_capacity::assess`), not a claim that
  // Ollama is unmetered - inventing a request-count number for a quota that isn't measured
  // in requests would produce a verdict (sufficient/tight/insufficient) about a dimension
  // Ollama doesn't actually meter that way.
  freeRpm: 0,
  freeRpd: 0,
  // Efficiency-first, not accuracy-first, and that is the opposite ranking from Groq's list
  // on purpose: Ollama's quota is spent in GPU-TIME, so a heavier model burns through a
  // session's or a week's allowance faster for the exact same one-hour-at-a-time workload.
  // gpt-oss is a deliberate choice here too - it is the same family Meridian already trusts
  // for strict-schema output via Groq, and it is present in Ollama's own catalogue
  // (confirmed live via `/v1/models` during this feature's research). The `-cloud` suffix is
  // Ollama's marker for "runs on their hardware, not yours"; both spellings are listed since
  // which one a given key's plan actually serves isn't knowable ahead of the real
  // `/v1/models` call `<CloudKeySetup>` makes with the user's own key.
  modelPreference: ['gpt-oss:20b-cloud', 'gpt-oss:20b', 'gpt-oss:120b-cloud', 'gpt-oss:120b'],
  // Ollama's catalogue is far more heterogeneous than Groq's curated chat-model list (it
  // mirrors the full public model registry), so this filter is broader than Groq's.
  nonChatFilter: ['whisper', 'tts', 'embed', 'bge', 'nomic', 'guard', 'prompt-guard'],
}

/**
 * Every no-subscription preset OFFERED for a brand-new key - the gate's free-path chooser and
 * the "add a new endpoint" tiles in Settings read only this list.
 *
 * GROQ IS DELIBERATELY NOT HERE. Groq's free tier has token-rate limits too tight for
 * Meridian's hourly pipeline calls - a real, observed production problem (hours silently
 * failing), not a hypothetical - so as of this list, nobody new is offered it. This is NOT
 * the same as deleting Groq support: `GROQ` itself, its trust copy, `pickGroqModel`, and the
 * backend registry (`add_custom_llm_provider`, `OpenAiCompatBackend`) are all untouched, and
 * exist precisely so someone who ALREADY has a Groq row keeps a fully working, fully
 * manageable endpoint - see `ALL_KNOWN_PRESETS` below, which is what renders THEIR tile.
 * `src/main.rs`'s poll loop separately raises a persistent notice for exactly these
 * users, pointing them at adding an Ollama key instead - see `llm.groq_deprecated`.
 */
export const CLOUD_PRESETS: CloudKeyPreset[] = [OLLAMA]

/**
 * Every preset this build knows how to DISPLAY, offered or not - a strict superset of
 * `CLOUD_PRESETS`. An already-configured endpoint on a retired preset (Groq, today) still
 * needs its name/logo/blurb to render its own tile and registry screen; `CLOUD_PRESETS`
 * alone would make that lookup fail the moment the preset stops being offered. Look up by
 * vendor with `presetForVendor`, never by re-checking `CLOUD_PRESETS.find(...)` for display
 * purposes - that conflates "known" with "offered", which is exactly the bug this avoids.
 */
export const ALL_KNOWN_PRESETS: CloudKeyPreset[] = [GROQ, OLLAMA]

/** The preset for `vendor`, whether or not it's currently offered - `undefined` for a vendor
 *  this build has never heard of (a hand-edited settings.json, or a future preset added to
 *  Rust before its frontend data lands). See `ALL_KNOWN_PRESETS`. */
export function presetForVendor(vendor: string): CloudKeyPreset | undefined {
  return ALL_KNOWN_PRESETS.find((p) => p.vendor === vendor)
}

/**
 * Choose the model to configure from the ids the endpoint reported, for a given preset.
 *
 * Asking the endpoint rather than hardcoding one id is the whole point of doing this at all -
 * a cloud catalogue turns over quickly, and a hardcoded id that has been retired fails at the
 * first real hour rather than at setup, the worst possible place. Matched as a PREFIX (not an
 * exact id) so a dated revision like `…-0905` still counts as its family.
 *
 * Falls back to the first usable id when nothing in `preset.modelPreference` is on offer, and
 * to `null` only when the list is empty or entirely non-chat - which the caller must treat as
 * "could not set this up", never as "use the default", since there is no default to use. This
 * fallback is deliberate: a vendor adds strict-schema models faster than this list is edited,
 * and the probe - not this list - is the real gate. A model the list has never heard of gets a
 * chance to prove itself and is refused honestly if it cannot.
 */
export function pickCloudModel(preset: CloudKeyPreset, available: string[]): string | null {
  const usable = available.filter(
    (m) => !preset.nonChatFilter.some((bad) => m.toLowerCase().includes(bad)),
  )
  for (const want of preset.modelPreference) {
    const hit = usable.find((m) => m.toLowerCase().startsWith(want.toLowerCase()))
    if (hit) return hit
  }
  return usable[0] ?? null
}

/** Groq-bound convenience wrapper, kept for the call sites and tests that already address it
 *  by name - a thin partial application of `pickCloudModel`, never a second implementation. */
export function pickGroqModel(available: string[]): string | null {
  return pickCloudModel(GROQ, available)
}

/** `GROQ_MODEL_PREFERENCE` re-exported under its old name for anything still importing it
 *  directly (see `GROQ.modelPreference`, the live copy). */
export const GROQ_MODEL_PREFERENCE: string[] = GROQ.modelPreference

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
