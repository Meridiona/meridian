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

import { stripComments } from './helpers/source'

const uiRoot = import.meta.dir + '/..'
const picker = readFileSync(uiRoot + '/components/ModelPicker.tsx', 'utf8')
const section = readFileSync(uiRoot + '/components/timeline/settings/IntelligenceSection.tsx', 'utf8')
const composer = readFileSync(uiRoot + '/components/timeline/llmlab/RunComposer.tsx', 'utf8')
const registry = readFileSync(uiRoot + '/lib/llm-providers.ts', 'utf8')
/** The setup wizard - the second surface that writes the provider. */
const setup = readFileSync(uiRoot + '/app/setup/page.tsx', 'utf8')



// ── Rule 1: a stored model override never rides onto another provider ────────
// The redesigned settings UI has NO per-provider model control - the model always follows the
// provider's own default. But `llm_provider_model` still exists on disk (older builds wrote it,
// and src/llm/config.rs still passes it into --model), so switching a built-in provider must
// CLEAR it: a value left from claude must not reach codex's -m.

/** onChange for a provider pick: the fields committed. A built-in clears the stored model. */
function switchFields(id: string): Record<string, unknown> {
  return id === 'custom'
    ? { llm_provider: id, llm_provider_custom_id: null }
    : { llm_provider: id, llm_provider_model: null }
}

describe('a stored model override is cleared when the provider changes', () => {
  it('clears llm_provider_model when switching to another built-in', () => {
    // The leak this stops: claude's "opus" reaching codex's -m.
    expect(switchFields('codex').llm_provider_model).toBeNull()
  })

  it('a custom switch carries the endpoint id, never the shared model override', () => {
    expect(switchFields('custom')).not.toHaveProperty('llm_provider_model')
    expect(switchFields('custom').llm_provider_custom_id).toBeNull()
  })
})

// Both surfaces that can change the provider must obey this, not just Settings. Asserting it
// against IntelligenceSection alone was false confidence: the wizard hand-rolled its own
// `{ llm_provider: id }` and so never cleared the override, and this suite stayed green because
// it did not read that file. Now the rule is pinned on the SHARED builder both of them call.
describe('every provider surface persists the choice through one builder', () => {
  const helper = stripComments(registry)

  it('does not write the shared model override for a custom endpoint', () => {
    // A custom endpoint's model lives on its own row; openai_compat sends ep.model and ignores
    // cfg.model, so the custom arm writes only the endpoint id, never the shared field.
    const start = helper.indexOf('return id === \'custom\'')
    const customArm = helper.slice(start, helper.indexOf(': {', start))
    expect(customArm).toContain('llm_provider_custom_id')
    expect(customArm).not.toContain('llm_provider_model')
  })

  it('clears the stored model on a built-in provider switch', () => {
    expect(helper).toMatch(/llm_provider: id, llm_provider_model: null/)
  })

  it('neither Settings nor the wizard hand-rolls the patch', () => {
    for (const src of [stripComments(section), stripComments(setup)]) {
      expect(src).toContain('providerChoiceFields(')
      // The literal the wizard used to write, which silently omitted the model clear.
      expect(src).not.toMatch(/\{\s*llm_provider:\s*id\s*\}/)
    }
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
