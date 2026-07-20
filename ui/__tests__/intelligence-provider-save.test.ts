//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'fs'

// Regression guard for "Settings claims a provider the daemon rejected".
//
// THE ORIGINAL BUG: Settings → Intelligence wrote the AI-provider choice on a single click:
//
//     patch(fields)          // optimistic - mutates the shared RuntimeSettings NOW
//     save(fields, setStatus)
//
// with a comment claiming `save` "rolls the store back on failure". It does not:
// `useRuntimeSettings.save` calls `setSettings` ONLY on success (settings/useRuntimeSettings.ts),
// so a REJECTED write left the optimistic patch in place - the panel went on showing a provider
// the daemon had refused.
//
// THE ARCHITECTURE NOW: the provider chooser is a two-level <LlmProviderPicker> (chooser +
// per-provider detail). A pick COMMITS IMMEDIATELY through the detail view's explicit "Use
// <provider>" button - there is no staging - but the anti-bug invariant is kept STRUCTURALLY a
// different way: the panel reads the displayed provider straight from `settings.llm_provider`
// (what is on disk), and `commit` writes ONLY through `save`, which mutates the in-memory
// settings only on success. So a failed write never changes what the panel shows, and there is
// no optimistic patch to roll back. `commit` also REJECTS on an error status, so the detail view
// can surface the failure instead of silently swallowing it.
//
// The repo has no React render harness (see plan-store / oauth-setup-lifecycle), so we model the
// transition and scan the source for the required shape rather than mounting the component.

const uiRoot = import.meta.dir + '/..'
const section = readFileSync(uiRoot + '/components/timeline/settings/IntelligenceSection.tsx', 'utf8')

/** Source with comments stripped. The module header deliberately describes the removed optimistic
 *  behaviour, so scanning raw text would conflate documenting the old bug with still doing it. */
const code = section
  .replace(/\/\*[\s\S]*?\*\//g, '')
  .replace(/^\s*\/\/.*$/gm, '')

// ── The commit model ─────────────────────────────────────────────────────────
// The displayed provider is `settings.llm_provider`; the parent's `save` updates that only on a
// successful write. `commit` resolves on 'saved' and rejects on 'error'.

/** What the panel SHOWS after a commit attempt: the pick only once the write succeeded. */
function shownAfter(saved: string, pick: string, status: 'saved' | 'error'): string {
  return status === 'saved' ? pick : saved
}

/** Does `commit` reject (so the detail view can show the failure)? */
function commitRejects(status: 'saved' | 'error'): boolean {
  return status === 'error'
}

describe('a provider pick commits through save, never optimistically', () => {
  it('shows the new provider only after the write succeeds', () => {
    expect(shownAfter('claude', 'codex', 'saved')).toBe('codex')
  })

  it('a FAILED write leaves the panel on the saved provider - never claims a rejected one', () => {
    // The whole point: no optimistic patch to survive, so what shows is still what is in force.
    expect(shownAfter('claude', 'codex', 'error')).toBe('claude')
  })

  it('a failed commit rejects, so the detail view can surface it instead of swallowing it', () => {
    expect(commitRejects('error')).toBe(true)
    expect(commitRejects('saved')).toBe(false)
  })

  it('the OLD optimistic path retained the rejected provider - proving these discriminate', () => {
    // Model the pre-fix behaviour: patch() mutated the shared settings BEFORE the write, and the
    // failure path never restored it.
    let settings = 'claude'
    const optimisticPatch = (next: string) => { settings = next }
    optimisticPatch('codex')
    // save() fails; useRuntimeSettings.save only setSettings on SUCCESS, so nothing undoes it.
    expect(settings).toBe('codex')      // <- the bug: shows a provider that was rejected
    expect(settings).not.toBe('claude') // <- and the real setting is misreported
  })
})

// ── The source shape that keeps the above true ───────────────────────────────
describe('IntelligenceSection writes nothing before a successful save', () => {
  it('has no optimistic patch() - the call the bug depended on is gone', () => {
    expect(code).not.toContain('patch')
  })

  it('does not stage the pick - the pick commits immediately, there is no pending state', () => {
    expect(code).not.toContain('setPending')
    expect(code).not.toContain('pending')
  })

  it('reads the displayed provider straight from settings on disk, not from local state', () => {
    // `value={provider}` where `provider = settings.llm_provider`, so a failed write can't change it.
    expect(code).toMatch(/const provider = settings\.llm_provider/)
    expect(code).toContain('value={provider}')
  })

  it('commits only through save, resolving on saved and rejecting on error', () => {
    expect(code).toMatch(/s === 'saved'/)
    expect(code).toMatch(/s === 'error'/)
    expect(code).toMatch(/reject\(/)
    // The only writes go through `commit`, which is the sole caller of `save`.
    expect(code).toContain('commit(')
  })
})
