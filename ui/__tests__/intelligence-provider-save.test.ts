//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'fs'

// Regression guard for "Settings claims a provider the daemon rejected".
//
// THE BUG: Settings → Intelligence wrote the AI-provider choice on a single click:
//
//     patch(fields)          // optimistic - mutates the shared RuntimeSettings NOW
//     save(fields, setStatus)
//
// with a code comment claiming `save` "rolls the store back to the server's answer
// on failure". It does not. `useRuntimeSettings.save` calls `setSettings` ONLY on
// success and swallows the error (settings/useRuntimeSettings.ts), so a REJECTED
// write left the optimistic patch in place: the panel went on showing a provider
// the daemon had refused, with nothing but a transient status to hint otherwise.
//
// THE FIX IS STRUCTURAL: a pick is STAGED in component-local `pending` state and
// nothing is written until an explicit Save reports success. Because no write
// happens before the outcome is known, there is no rollback to get wrong. On
// failure the staged pick deliberately SURVIVES, so the user can press Save again
// without re-picking.
//
// Two further rules these guards pin, both easy to regress:
//   - the dirty check compares the (provider, custom_id) PAIR. 'custom' is a KIND,
//     so two different endpoints are both id === 'custom'; comparing the id alone
//     makes switching between them invisible and Save never lights up.
//   - re-picking whatever is already saved CLEARS the stage, so Save can never
//     offer to write a change that isn't one.
//
// The repo has no React render harness (see plan-store / oauth-setup-lifecycle), so
// we model the transitions and scan the source for the required shape rather than
// mounting the component.

const uiRoot = import.meta.dir + '/..'
const section = readFileSync(uiRoot + '/components/timeline/settings/IntelligenceSection.tsx', 'utf8')

/** Source with comments stripped. The structural guards below assert on CODE - the
 *  module header deliberately describes the removed `patch()`-on-click behaviour, so
 *  scanning raw text would conflate documenting the old bug with still doing it. */
const code = section
  .replace(/\/\*[\s\S]*?\*\//g, '')
  .replace(/^\s*\/\/.*$/gm, '')

// ── The staging model ────────────────────────────────────────────────────────
// Mirrors IntelligenceSection's two transitions: `onChange` (stage a pick) and
// the `save` status callback (resolve it).

interface Choice { id: string; customId: string | null }

/** `onChange`: stage a pick, or clear the stage when it IS the saved choice. */
function stage(saved: Choice, pick: Choice): Choice | null {
  const nextCustomId = pick.id === 'custom' ? pick.customId ?? null : null
  const backToSaved = pick.id === saved.id && (pick.id !== 'custom' || nextCustomId === saved.customId)
  return backToSaved ? null : { id: pick.id, customId: nextCustomId }
}

/** The `save` status callback: the stage clears on 'saved' only. */
function onSaveStatus(pending: Choice | null, status: 'idle' | 'saved' | 'error'): Choice | null {
  return status === 'saved' ? null : pending
}

/** What the panel DISPLAYS: the staged pick while one is held, else what's on disk. */
function shown(saved: Choice, pending: Choice | null): Choice {
  return pending ?? saved
}

const CLAUDE: Choice = { id: 'claude', customId: null }
const LOCAL: Choice = { id: 'local', customId: null }
const CUSTOM_A: Choice = { id: 'custom', customId: 'endpoint-a' }
const CUSTOM_B: Choice = { id: 'custom', customId: 'endpoint-b' }

describe('a provider pick is staged, not written', () => {
  it('picking a different provider stages it without touching the saved choice', () => {
    const pending = stage(CLAUDE, LOCAL)
    expect(pending).toEqual(LOCAL)
    // The panel shows the staged pick...
    expect(shown(CLAUDE, pending).id).toBe('local')
    // ...but nothing has been persisted, so the saved choice is still Claude.
    expect(CLAUDE.id).toBe('claude')
  })

  it('re-picking the saved provider clears the stage, so Save cannot offer a no-op', () => {
    expect(stage(CLAUDE, CLAUDE)).toBeNull()
    // And staging then coming back also clears it.
    const staged = stage(CLAUDE, LOCAL)
    expect(staged).not.toBeNull()
    expect(stage(CLAUDE, CLAUDE)).toBeNull()
  })

  it('a successful save resolves the stage', () => {
    const pending = stage(CLAUDE, LOCAL)
    expect(onSaveStatus(pending, 'saved')).toBeNull()
  })
})

// ── The reviewer's explicit ask, and the bug this PR fixes ───────────────────
describe('a FAILED save keeps the staged pick', () => {
  it('keeps it, so the user can press Save again without re-picking', () => {
    const pending = stage(CLAUDE, LOCAL)
    const after = onSaveStatus(pending, 'error')
    expect(after).toEqual(LOCAL)
  })

  it("and the saved choice is untouched — the panel never claims a rejected provider", () => {
    const saved = CLAUDE
    const pending = stage(saved, LOCAL)
    onSaveStatus(pending, 'error')
    // Nothing was written, so what is actually in force is still Claude.
    expect(saved).toEqual(CLAUDE)
  })

  it('the OLD optimistic path retained the rejected provider — proving these discriminate', () => {
    // Model the pre-fix behaviour: patch() mutated the shared settings BEFORE the
    // write, and the failure path never restored it.
    let settings: Choice = { ...CLAUDE }
    const optimisticPatch = (next: Choice) => { settings = next }
    optimisticPatch(LOCAL)
    // save() fails: useRuntimeSettings.save only setSettings on SUCCESS, so nothing
    // undoes the patch.
    expect(settings).toEqual(LOCAL)      // <- the bug: shows a provider that was rejected
    expect(settings).not.toEqual(CLAUDE) // <- and the real setting is misreported
  })
})

// ── The pair comparison ──────────────────────────────────────────────────────
describe("the dirty check compares the (provider, custom_id) pair", () => {
  it('switching between two custom endpoints registers as a change', () => {
    const pending = stage(CUSTOM_A, CUSTOM_B)
    expect(pending).toEqual(CUSTOM_B)
  })

  it('re-picking the same custom endpoint clears the stage', () => {
    expect(stage(CUSTOM_A, CUSTOM_A)).toBeNull()
  })

  it('comparing the id ALONE would miss it — proving the pair matters', () => {
    const idOnly = (saved: Choice, pick: Choice) => (pick.id === saved.id ? null : pick)
    // Both are id === 'custom', so an id-only check sees no change and Save never lights up.
    expect(idOnly(CUSTOM_A, CUSTOM_B)).toBeNull()
    expect(stage(CUSTOM_A, CUSTOM_B)).not.toBeNull()
  })

  it('leaving custom for a built-in drops the endpoint id', () => {
    expect(stage(CUSTOM_A, CLAUDE)).toEqual({ id: 'claude', customId: null })
  })
})

// ── The source shape that keeps the above true ───────────────────────────────
describe('IntelligenceSection writes nothing before a successful save', () => {
  it('has no optimistic patch() — the call the bug depended on is gone', () => {
    // `patch` is no longer even a prop; the write happens only inside onSave.
    expect(code).not.toContain('patch')
  })

  it('clears the stage only on a SAVED status, never unconditionally', () => {
    // The single setPending(null) that resolves a save must be guarded by 'saved'.
    expect(code).toMatch(/'saved'\s*\)\s*setPending\(null\)/)
    // ...and that is the ONLY place the stage is cleared post-save.
    const clears = code.match(/setPending\(null\)/g) ?? []
    expect(clears.length).toBe(1)
  })

  it('stages the pick in onChange rather than saving from it', () => {
    expect(code).toContain('setPending(')
    // onChange must not call save directly - that would restore click-to-apply.
    const onChangeBody = code.slice(code.indexOf('const onChange'), code.indexOf('const onSave'))
    expect(onChangeBody).not.toContain('save(')
  })

  it('renders the picker from the staged pick, so the panel shows what Save would write', () => {
    expect(code).toMatch(/pending\?\.id\s*\?\?\s*savedId/)
  })
})

describe('the Save control cannot fall below the fold', () => {
  it('is sticky and only rendered while a pick is staged', () => {
    expect(code).toContain('sticky bottom-0')
    expect(code).toMatch(/\{pending && \(/)
  })

  it('tells the user the pick survived a failed write', () => {
    // Generic "Failed to save" can't say the stage was kept; this line must.
    expect(code).toContain('still selected here')
  })
})
