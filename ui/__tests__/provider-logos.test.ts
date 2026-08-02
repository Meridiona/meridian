//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'fs'

// The provider marks are the REAL logos now, and the path data therefore exists twice: once
// in `public/logos/<name>.svg` (the downloaded file, with its provenance recorded in that
// folder's README) and once inline in the component, because every mark is filled with
// `currentColor` and an <img> cannot inherit that.
//
// Two copies of anything drift. These pin them together, so a hand-tweak in the component
// fails here rather than quietly putting a not-quite-right logo back in front of someone
// being asked to recognise their own subscription.

const uiRoot = import.meta.dir + '/..'
const component = readFileSync(`${uiRoot}/components/LlmProviderLogos.tsx`, 'utf8')
const readme = readFileSync(`${uiRoot}/public/logos/README.md`, 'utf8')

const MARKS = [
  // `flipsOnDark` marks the brands whose logo IS black - they must be redefined under the
  // `ink` theme or they disappear into a dark card. Claude's coral and Groq's orange carry
  // across both, so a second definition for them would just be a place to drift.
  { file: 'claude', constant: 'CLAUDE', cssVar: '--logo-claude', flipsOnDark: false },
  { file: 'openai', constant: 'OPENAI', cssVar: '--logo-openai', flipsOnDark: true },
  { file: 'cursor', constant: 'CURSOR', cssVar: '--logo-cursor', flipsOnDark: true },
  { file: 'groq', constant: 'GROQ', cssVar: '--logo-groq', flipsOnDark: false },
]

/** The single `d` and `viewBox` out of a saved mark. */
function fromFile(name: string) {
  const svg = readFileSync(`${uiRoot}/public/logos/${name}.svg`, 'utf8')
  const paths = svg.match(/<path\b/g) ?? []
  return {
    svg,
    paths: paths.length,
    d: svg.match(/<path d="([^"]+)"/)?.[1] ?? null,
    viewBox: svg.match(/viewBox="([^"]+)"/)?.[1] ?? null,
  }
}

describe('the saved logo files', () => {
  for (const { file } of MARKS) {
    it(`${file}.svg is one path on a square box, filled with currentColor`, () => {
      const m = fromFile(file)
      // One path, because the component renders exactly one. A multi-path file would
      // silently lose everything after the first.
      expect(m.paths).toBe(1)
      expect(m.d).toBeTruthy()
      // currentColor is what lets the tiles recolour the mark for idle/in-use and for
      // light/dark. A baked-in fill would look wrong in one of the four combinations.
      expect(m.svg).toContain('fill="currentColor"')
      // Square, or the mark distorts when rendered at width === height.
      const [, , w, h] = (m.viewBox ?? '').split(/\s+/).map(Number)
      expect(w).toBeCloseTo(h, 3)
    })
  }
})

describe('the component copies match the files byte for byte', () => {
  for (const { file, constant, cssVar } of MARKS) {
    it(`${constant} is exactly what is in ${file}.svg`, () => {
      const m = fromFile(file)
      const decl = component.match(
        new RegExp(`const ${constant} = \\{ fill: 'var\\((--[a-z-]+)\\)', viewBox: '([^']+)', d: '([^']+)' \\}`),
      )
      expect(decl).not.toBeNull()
      expect(decl![1]).toBe(cssVar)
      expect(decl![2]).toBe(m.viewBox!)
      expect(decl![3]).toBe(m.d!)
    })
  }
})

describe('the marks wear their own brand colour', () => {
  const css = readFileSync(`${uiRoot}/app/globals.css`, 'utf8')
  const ink = css.slice(css.indexOf('html[data-theme="ink"] {'))

  it('none of them inherits the app accent any more', () => {
    // They used to be `currentColor` so a selected tile could tint them violet - which made
    // every logo the same colour and defeated the point of showing a real one. Selection is
    // carried by the tile's ring and its IN USE badge instead.
    for (const { constant } of MARKS) {
      const decl = component.slice(component.indexOf(`const ${constant} =`))
      expect(decl.slice(0, 60)).not.toContain('currentColor')
    }
  })

  for (const { cssVar, flipsOnDark } of MARKS) {
    it(`${cssVar} is defined once, and ${flipsOnDark ? 'flips' : 'does not flip'} under ink`, () => {
      expect(css).toContain(`${cssVar}:`)
      // A black mark that is never redefined for the dark theme is invisible on it - the
      // failure is silent and only shows up on someone else's machine.
      expect(ink.includes(`${cssVar}:`)).toBe(flipsOnDark)
    })
  }

  it('keeps the brand colours out of the theme palettes', () => {
    // A brand colour is not ours to restyle per theme, so the base four are declared in a
    // BARE `:root {` of their own - not folded into the lilac/blush palette block, where
    // the next person adding a theme would reasonably assume they are fair game. `ink`
    // overrides only the two that have to flip.
    const decl = css.slice(0, css.indexOf('--logo-claude'))
    const openedBy = decl.slice(decl.lastIndexOf('{') - 80, decl.lastIndexOf('{') + 1)
    expect(openedBy.trim().endsWith(':root {')).toBe(true)
  })
})

describe('provenance is recorded', () => {
  it('every mark has a source and a licence in the README', () => {
    // Shipping someone else's brand asset without saying where it came from or under what
    // terms is the kind of thing that is very hard to reconstruct a year later.
    for (const { file } of MARKS) expect(readme).toContain(`${file}.svg`)
    expect(readme).toContain('CC0-1.0')
    expect(readme).toContain('groq.com/favicon.svg')
    // Trademark is a separate question from the file's licence, and the distinction is the
    // whole reason this use is defensible.
    expect(readme).toMatch(/trademark/i)
    expect(readme).toMatch(/nominativ/i)
  })
})

describe('nothing looks chosen before the user chooses', () => {
  const picker = readFileSync(`${uiRoot}/components/LlmProviderPicker.tsx`, 'utf8')

  it('suppresses the selected ring under the gate', () => {
    // `value` is never empty - it falls back to Claude (LlmProvider::default()), which is
    // right for the resolver and a lie here: the user reached this screen precisely because
    // no provider works. A highlighted tile says a choice has been made and there is
    // nothing to do, and it lands on whichever provider is stored - at worst the one that
    // just failed.
    expect(picker).toContain('const isSelected = (!gate || committedId === id) && value === id')
  })

  it('gives every tile a call to action instead', () => {
    // Three equal cards with no selection and no prompt do not read as a question waiting
    // on an answer.
    expect(picker).toContain("action={gate ? 'Choose' : undefined}")
  })

  it('but DOES show the one they chose, once they have chosen it', () => {
    // Coming back to the grid after setting a provider up must look different from
    // arriving at it. `committedId` is the record of a pick made in THIS flow.
    expect(picker).toContain('setCommittedId(id)')
    expect(picker).toContain('await onChange(id, customId)')
  })

  const tile = picker.slice(picker.indexOf('function ChooserTile'), picker.indexOf('export interface LlmProviderPickerProps'))

  it('marks a WORKING chosen tile green, not the app accent', () => {
    // Violet is Meridian's accent and is already on every interactive thing here, so a
    // violet ring reads as "clickable" rather than "this is the one". Green matches the
    // IN USE badge the same tile already carries.
    expect(tile).toContain('const live = selected && !warning')
    expect(tile).toContain("live ? 'var(--color-state-approved)' : 'var(--t-ctrl-border)'")
    expect(tile).not.toContain("selected ? 'var(--color-state-proposal)'")
  })

  it('and NOTHING else on the grid is ever coloured', () => {
    // Two earlier versions failed in the same direction. Colouring on `selected` alone put a
    // green box and a green glow around a tile whose own badge said ERROR - and `selected`
    // stays true when a provider breaks, so that is what a user saw the moment one stopped
    // working. Colouring problems amber then turned the grid orange in the two most ordinary
    // states there are: a fresh install with no CLIs, and a signed-out provider.
    expect(tile).not.toContain('--color-state-pending')
    // The tile's own message is muted body text, like any other subtitle.
    expect(tile).toContain("{warning ? warning.message : subtitle}")
  })

  it('never says anything connected-sounding about a provider that is not connected', () => {
    // "SELECTED" was read as "connected" - it is a fact about settings.json that a user
    // reasonably takes as a fact about the app. The pill says the opposite thing instead.
    expect(tile).toContain("{live ? 'IN USE' : 'NOT CONNECTED'}")
  })
})
