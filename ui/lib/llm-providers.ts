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
}

export const LLM_PROVIDERS: LlmProviderMeta[] = [
  {
    id: 'local',
    name: 'On-device',
    blurb: 'Runs entirely on your Mac. Nothing you do leaves the machine.',
    kind: 'local',
    bin: null,
  },
  {
    id: 'claude',
    name: 'Claude Code',
    blurb: 'Uses your Claude subscription through the claude CLI.',
    kind: 'cli',
    bin: 'claude',
    installHint: 'npm i -g @anthropic-ai/claude-code',
    installUrl: 'https://claude.com/claude-code',
  },
  {
    id: 'codex',
    name: 'Codex',
    blurb: 'Uses your ChatGPT subscription through the codex CLI.',
    kind: 'cli',
    bin: 'codex',
    installHint: 'npm i -g @openai/codex',
    installUrl: 'https://developers.openai.com/codex/cli',
  },
  {
    id: 'cursor',
    name: 'Cursor',
    blurb: 'Uses your Cursor subscription through the cursor-agent CLI.',
    kind: 'cli',
    bin: 'cursor-agent',
    installHint: 'curl https://cursor.com/install -fsS | bash',
    installUrl: 'https://cursor.com/cli',
  },
  {
    id: 'copilot',
    name: 'GitHub Copilot',
    blurb: 'Uses your Copilot subscription through the copilot CLI.',
    kind: 'cli',
    bin: 'copilot',
    installHint: 'npm i -g @github/copilot',
    installUrl: 'https://github.com/features/copilot/cli',
  },
]

/** The default. On-device, because privacy is the point and it is always present. */
export const DEFAULT_LLM_PROVIDER: LlmProviderId = 'local'

export function llmProvider(id: LlmProviderId): LlmProviderMeta {
  return LLM_PROVIDERS.find(p => p.id === id) ?? LLM_PROVIDERS[0]
}
