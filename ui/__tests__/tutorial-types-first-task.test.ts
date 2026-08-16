//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The planner's first beat writes the note itself instead of waiting for the
// user to invent one.
//
// It used to ring an empty box and wait up to three minutes. Nothing has
// happened by that point in the tour: the user has not seen Meridian draft
// anything, does not know what a scruffy one-liner buys them over a title, and
// is being asked to make up an example in a field whose format they are
// guessing at - before the tour will move at all.
//
// Typing it is not cosmetic. The beat straight after asks them to press "Draft
// with AI", and that press is the first time the product does anything: one
// scruffy line becoming a titled, described task. A user stuck at the blank box
// never reaches it.
//
// Source-scanning: driving a React-controlled field from outside React is the
// kind of thing that fails by doing NOTHING - the character lands for one frame
// and the next render wipes it, with no error anywhere. There is no DOM here to
// assert against, and a mocked one would not have React's value tracker, which
// is the exact part that breaks.

import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'

const read = (...p: string[]) => readFileSync(join(import.meta.dir, '..', ...p), 'utf8')

const script = read('components', 'tutorial', 'script.ts')
const engine = read('components', 'tutorial', 'engine.ts')
const hook = read('components', 'tutorial', 'useTutorial.tsx')
const composer = read('components', 'plan', 'TaskComposer.tsx')

describe('the walkthrough writes the first task rather than waiting for one', () => {
  it('types into the note instead of waiting on the user', () => {
    expect(script).toMatch(/await s\.type\('\[data-tour="task-note"\]', FIRST_TASK_NOTE\)/)
    // The old three-minute wait is gone, not merely bypassed. Left in place it
    // would sit after the typing and block on a field that is already full.
    expect(script).not.toMatch(/waitForValue\('\[data-tour="task-note"\]'/)
  })

  it('writes a real task, not the field\'s own placeholder', () => {
    // This lands in the user's REAL plan - part one runs against their own day,
    // not the example - so a fiction leaves a made-up task on a real board. And
    // echoing the placeholder would make the whole beat read as a canned demo.
    const note = script.match(/^const FIRST_TASK_NOTE = '([^']+)'/m)?.[1]
    expect(note).toBeTruthy()
    const placeholder = composer.match(/placeholder="([^"]*login[^"]*)"/)?.[1] ?? 'fix the flaky login test…'
    expect(note).not.toBe(placeholder.replace('…', ''))
    expect(note!.toLowerCase()).toContain('meridian')
  })

  it('gives the model enough to draft a title that clears the floor', () => {
    // `canCreate` needs MIN_TITLE_WORDS (4). Beat 2c-2 handles a short drafted
    // title gracefully - it names what is missing and waits on the real gate -
    // but that branch asks for exactly the typing this change removed. While
    // every user wrote their own note, landing on it was a per-user lottery;
    // with ONE note it is all-or-nothing for the whole fleet, and the example
    // recorded in that beat is an onboarding note drafting down to two words.
    //
    // The model's output cannot be asserted here. The INPUT's substance can, and
    // that is the thing a future edit would quietly erode.
    const note = script.match(/^const FIRST_TASK_NOTE = '([^']+)'/m)![1]
    const words = note.trim().split(/\s+/).filter(Boolean)
    const filler = new Set(['and', 'the', 'a', 'an', 'through', 'to', 'of', 'on', 'my', 'with', 'for', 'in'])
    const content = words.filter(w => !filler.has(w.toLowerCase()))
    // Comfortably clear of the four-word floor even after a faithful title drops
    // the filler - not a padding check, a "do not shorten this to two words" one.
    expect(content.length).toBeGreaterThanOrEqual(5)
  })

  it('still hands the Draft press to the user', () => {
    // The typing removes a wait the user gained nothing from. It must not remove
    // the press they gain everything from - that click is the first time they
    // see the product do work.
    const typed = script.indexOf("s.type('[data-tour=\"task-note\"]'")
    const draft = script.indexOf("await s.waitForClick('[data-tour=\"task-draft\"]'")
    expect(typed).toBeGreaterThan(-1)
    expect(draft).toBeGreaterThan(typed)
    // Not clicked FOR them anywhere in that stretch.
    expect(script.slice(typed, draft)).not.toContain('s.click()')
  })
})

describe('the typing drives the real field', () => {
  const start = hook.indexOf('type: async (sel, text)')
  const end = hook.indexOf('value: (sel)', start)   // searched FROM the start, so
  // a reorder that put `value:` above `type:` fails here instead of silently
  // widening the slice to most of the file and passing on anything.
  if (start < 0 || end < start) throw new Error('type/value primitives not found in expected order')
  const impl = hook.slice(start, end)
  /** The same slice with `//` comments dropped. The comments here NAME the
   *  anti-pattern they exist to warn about, so a check for it reading the raw
   *  text fails on a correct file. */
  const code = impl.replace(/^\s*\/\/.*$/gm, '')

  it('goes through the native setter, not a plain assignment', () => {
    // React tracks the last value it rendered and compares it against the node's
    // own property. `el.value = x` updates neither its tracker nor its state, so
    // the text shows for one frame, `onChange` never fires, and the next render
    // wipes it. Silently - which is why this is pinned.
    expect(impl).toContain("Object.getOwnPropertyDescriptor(proto, 'value')?.set")
    expect(impl).toContain("new Event('input', { bubbles: true })")
    expect(code).not.toMatch(/\bel\.value\s*=/)
  })

  it('does not reach into the composer store instead', () => {
    // `setNote` from useTaskComposer would be one line and would quietly stop
    // being a demonstration - the same reasoning that has `demoDrag` pressing the
    // product's own "+ Add" rather than editing the plan.
    expect(hook).not.toContain('setNote')
    expect(engine).toContain('type(selector: string, text: string): Promise<boolean>')
  })

  it('types a character at a time, and can be skipped mid-sentence', () => {
    // One assignment lands the sentence in a single frame, which looks like a
    // pre-filled form - and a pre-filled form teaches nothing about where the
    // words go.
    expect(impl).toMatch(/for \(let i = 1; i <= text\.length; i\+\+\)/)
    // `sleep(ms, sig)` rejects Aborted, so Skip unwinds the script from inside
    // the loop exactly as it does from any other await. A bare timeout would
    // keep typing into a walkthrough the user has already left.
    expect(impl).toMatch(/await sleep\(TYPE_MS, sig\)/)
  })

  it('answers false for a field that is not there', () => {
    // Same contract as every other primitive that targets an element. Throwing
    // would abort the whole tour over one missing box.
    expect(impl).toMatch(/if \(!el\) return false/)
  })
})
