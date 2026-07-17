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
// coding-agent CLI that is already installed and signed in. `local` runs the on-device
// MLX model - nothing leaves the machine.

export type LlmProviderId = 'claude' | 'codex' | 'cursor' | 'copilot' | 'local'

export interface LlmProviderMeta {
  id: LlmProviderId
  name: string
  /** One-line "what this is", shown on the card. */
  blurb: string
  /** 'cli' → shells out to an installed binary; 'local' → the on-device model. */
  kind: 'cli' | 'local'
  /** The executable we look for. null for the on-device model (it is an HTTP call). */
  bin: string | null
  /** Shown when the CLI is not installed - the command that installs it. */
  installHint?: string
  installUrl?: string
  /**
   * Where the user opts their prompts OUT of model training. This is ACCOUNT-LEVEL for
   * every provider (a subprocess can't flip it - Meridian only disables telemetry per
   * call), so the UI links it rather than pretending to toggle it. null for on-device,
   * which never sends anything anywhere.
   */
  privacyUrl: string | null
  /** Human label for the account setting the privacyUrl leads to. */
  privacyLabel?: string
}

// Order IS the recommendation: coding agents first (frontier-model accuracy), on-device
// last as the private, always-available fallback. The picker renders them in this order.
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
  },
  {
    id: 'local',
    name: 'On-device',
    blurb: 'Runs entirely on your Mac. Nothing you do leaves the machine.',
    kind: 'local',
    bin: null,
    privacyUrl: null,
  },
]

/**
 * The stored default. On-device, because it is the only backend guaranteed present and
 * needs no login - so a fresh install always works. The picker still lists the coding
 * agents first (see the ordering comment on `LLM_PROVIDERS`), but the safe default it
 * falls back to is local.
 */
export const DEFAULT_LLM_PROVIDER: LlmProviderId = 'local'

/**
 * The rough share of a plan's usage limit that Meridian's hourly summaries consume - one
 * short request per active hour. Surfaced to reassure users that pointing Meridian at
 * their coding-agent subscription won't eat into their own coding headroom.
 */
export const USAGE_FOOTPRINT_NOTE =
  'Meridian sends about one short request per active hour - well under 1% of your plan’s usage limits, so it won’t eat into your own coding.'

export function llmProvider(id: LlmProviderId): LlmProviderMeta {
  // Fall back to on-device, never to LLM_PROVIDERS[0] (now a CLI) - an unknown id must
  // resolve to the always-safe default, matching the Rust resolver.
  return LLM_PROVIDERS.find(p => p.id === id)
    ?? LLM_PROVIDERS.find(p => p.id === 'local')
    ?? LLM_PROVIDERS[LLM_PROVIDERS.length - 1]
}
