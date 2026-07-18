//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'fs'

// Guards for the model pickers that replaced free-text model entry.
//
// Three rules here are load-bearing and every one of them is invisible at the type
// level, so nothing but a test stops a later edit from quietly undoing them:
//
//   1. THE MODEL OVERRIDE IS SCOPED TO ONE PROVIDER. `src/llm/detect.rs` clears
//      cfg.model when testing any provider that is not the selected one, precisely
//      so one CLI's model string cannot end up in another's --model. The settings UI
//      has to honour the same rule: switching provider must DROP the staged model.
//      Carrying it over would hand `claude`'s "opus" to codex's -m, which fails only
//      at run time, as a nonzero CLI exit an hour later.
//
//   2. A PROVIDER THAT DISCARDS THE MODEL MUST NOT FAN OUT. copilot's argv is built
//      without a model flag and never reads cfg.model (src/llm/copilot.rs), so a
//      `copilot:<model>` Lab variant would render a column promising a comparison the
//      run cannot actually make - two identical results labelled as different models.
//
//   3. FREE TEXT IS NEVER REMOVED. Nothing validates a model string anywhere in the
//      pipeline; it goes verbatim into argv or a JSON body. The curated lists are a
//      convenience, so a stale list must never be able to block a model that works.
//      This is the whole reason `cursor` ships an empty list rather than guesses.
//
// The repo has no React render harness (see intelligence-provider-save), so we model
// the transitions and scan the source for the required shape rather than mounting.

const uiRoot = import.meta.dir + '/..'
const picker = readFileSync(uiRoot + '/components/ModelPicker.tsx', 'utf8')
const section = readFileSync(uiRoot + '/components/timeline/settings/IntelligenceSection.tsx', 'utf8')
const composer = readFileSync(uiRoot + '/components/timeline/llmlab/RunComposer.tsx', 'utf8')
const registry = readFileSync(uiRoot + '/lib/llm-providers.ts', 'utf8')

/** Source with comments stripped - these guards assert on CODE, and the headers above
 *  each file describe the very behaviour being guarded against. */
function stripComments(src: string): string {
  return src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/^\s*\/\/.*$/gm, '')
}

// ── Rule 1: the staging model ────────────────────────────────────────────────
// Mirrors IntelligenceSection's `onChange` / `onModelChange`.

interface Choice { id: string; customId: string | null; model: string }

/** `onChange`: stage a provider pick, dropping the model unless we land back on the
 *  saved provider (where the saved model is what's on disk). */
function stageProvider(saved: Choice, pickId: string, pickCustomId: string | null = null): Choice | null {
  const nextCustomId = pickId === 'custom' ? pickCustomId : null
  const nextModel = pickId === saved.id ? saved.model : ''
  const backToSaved = pickId === saved.id && (pickId !== 'custom' || nextCustomId === saved.customId)
  return backToSaved ? null : { id: pickId, customId: nextCustomId, model: nextModel }
}

/** `onModelChange`: stage a model, collapsing to null once the whole triple matches disk. */
function stageModel(saved: Choice, pending: Choice | null, next: string): Choice | null {
  const staged = { ...(pending ?? saved), model: next }
  const settled = staged.id === saved.id
    && (staged.id !== 'custom' || staged.customId === saved.customId)
    && staged.model === saved.model
  return settled ? null : staged
}

describe('the model override is scoped to its provider', () => {
  const saved: Choice = { id: 'claude', customId: null, model: 'opus' }

  it('drops the staged model when the provider changes', () => {
    // The leak this exists to stop: claude's "opus" reaching codex's -m.
    expect(stageProvider(saved, 'codex')?.model).toBe('')
  })

  it('restores the saved model when the provider comes back', () => {
    const away = stageProvider(saved, 'codex')
    expect(away?.model).toBe('')
    // Re-picking the saved provider clears the stage entirely, so what renders is
    // the value on disk rather than the '' we passed through on the way out.
    expect(stageProvider(saved, 'claude')).toBeNull()
  })

  it('keeps a model staged across an unrelated re-render', () => {
    const staged = stageModel(saved, null, 'haiku')
    expect(staged).toEqual({ id: 'claude', customId: null, model: 'haiku' })
  })

  it('clears the stage when the model returns to what is saved', () => {
    const staged = stageModel(saved, null, 'haiku')
    expect(stageModel(saved, staged, 'opus')).toBeNull()
  })

  it('treats clearing the model to the provider default as a real change', () => {
    // '' is a legitimate value ("use the provider's default"), not a no-op, so it
    // must stage and be savable - it is written as null.
    expect(stageModel(saved, null, '')).toEqual({ id: 'claude', customId: null, model: '' })
  })
})

describe('the settings section persists the override correctly', () => {
  const code = stripComments(section)

  it('writes llm_provider_model for a CLI provider', () => {
    expect(code).toContain('llm_provider_model')
  })

  it('stores an empty model as null, matching Option<String>/None in Rust', () => {
    expect(code).toMatch(/llm_provider_model:\s*pending\.model\s*\|\|\s*null/)
  })

  it('does not write the shared override for a custom endpoint', () => {
    // A custom endpoint's model lives on its own row; openai_compat sends ep.model
    // and ignores cfg.model, so writing the shared field would be a dead setting.
    const customBranch = code.slice(code.indexOf("pending.id === 'custom'"))
    const arm = customBranch.slice(0, customBranch.indexOf(':'))
    expect(arm).not.toContain('llm_provider_model')
  })

  it('only offers the picker for a backend that passes the model through', () => {
    expect(code).toContain('supportsModelOverride')
  })

  it('actually resets the model on a provider switch', () => {
    // The transition tests above model this rule; this one ties it to the SOURCE, so
    // the guard can't keep passing against a component that stopped honouring it.
    expect(code).toMatch(/id === savedId\s*\?\s*savedModel\s*:\s*''/)
  })
})

describe('a provider that discards the model never fans out', () => {
  const code = stripComments(composer)

  it('gates the Lab variant fan-out on supportsModelOverride', () => {
    // Without this, a stale string in state still emits `copilot:<model>` columns.
    // Bound the slice from the filter forward - `customVariantId` also appears in the
    // import line above it, which would make this an empty (vacuously passing) slice.
    const start = code.indexOf('LLM_PROVIDERS.filter')
    const variantBlock = code.slice(start, code.indexOf('customVariantId', start))
    expect(variantBlock).toContain('supportsModelOverride')
  })

  it('marks copilot as not supporting a model override', () => {
    const copilotBlock = registry.slice(registry.indexOf("id: 'copilot'"))
    expect(copilotBlock).toMatch(/supportsModelOverride:\s*false/)
  })

  it('marks the model-passing CLIs as supporting it', () => {
    for (const id of ['claude', 'codex', 'cursor']) {
      const block = registry.slice(registry.indexOf(`id: '${id}'`))
      expect(block).toMatch(/supportsModelOverride:\s*true/)
    }
  })
})

describe('free text is always reachable', () => {
  const code = stripComments(picker)

  it('keeps an escape hatch in the single-model control', () => {
    expect(code).toContain('Custom model…')
  })

  it('falls back to free text when there is nothing curated to offer', () => {
    // cursor ships an empty list on purpose; an empty list must degrade to a text
    // field rather than render a dropdown with no options.
    expect(code).toMatch(/models\.length === 0/)
  })

  it('keeps a text input in the Lab multi-select', () => {
    const multi = code.slice(code.indexOf('export function ModelMultiSelect'))
    expect(multi).toContain('<input')
  })

  // The escape hatch was BROKEN on first submission and the string-scan guards above did
  // not catch it: picking "Custom model…" clears the value, and an empty value against a
  // non-empty list is indistinguishable from "nothing chosen yet", so the derived flag went
  // straight back to false and the text input never rendered. Model the derivation to pin
  // the behaviour, not just the presence of the option.
  const isFreeText = (customMode: boolean, models: string[], value: string) =>
    customMode || models.length === 0 || (value !== '' && !models.includes(value))

  it('stays in free text after picking Custom model on a non-empty list', () => {
    // customMode must be held explicitly - this is the case that regressed.
    expect(isFreeText(true, ['opus', 'sonnet'], '')).toBe(true)
  })

  it('would NOT stay in free text if the mode were derived from the value alone', () => {
    // Documents why the explicit flag exists: without it this is the failing case.
    expect(isFreeText(false, ['opus', 'sonnet'], '')).toBe(false)
  })

  it('leaves free text once a listed model is picked', () => {
    expect(isFreeText(false, ['opus', 'sonnet'], 'opus')).toBe(false)
  })

  it('holds the explicit custom-entry flag in component state', () => {
    expect(code).toContain('customMode')
    expect(code).toMatch(/setCustomMode\(custom\)/)
  })

  it('shows an unlisted stored model rather than snapping it to a listed one', () => {
    // A value we don't carry must switch the control to free text, or revisiting the
    // page would silently rewrite the user's model.
    expect(code).toMatch(/value !== ''\s*&&\s*!known/)
  })
})

describe('the curated registry stays honest', () => {
  it('ships cursor with no invented model ids', () => {
    // cursor-agent takes --model, but no verifiable value exists; a wrong one is
    // accepted silently and fails only when the CLI runs.
    const block = registry.slice(registry.indexOf("id: 'cursor'"), registry.indexOf("id: 'copilot'"))
    expect(block).toMatch(/models:\s*\[\]/)
  })

  it('leaves custom endpoints to live enumeration', () => {
    const block = registry.slice(registry.indexOf('CUSTOM_PROVIDER_META'))
    expect(block).toMatch(/models:\s*\[\]/)
  })
})
