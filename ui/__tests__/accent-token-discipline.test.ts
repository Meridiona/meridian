//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

// Two token rules that fail SILENTLY when broken, which is why they are pinned.
//
// # Rule 1 - `--color-state-proposal` is a state, not the brand
// It is a theme-INDEPENDENT semantic colour meaning "a proposal exists". It also
// happens to be violet, so it spread across the UI as a stand-in for the brand
// accent - 174 uses at its peak, on buttons, borders, links and tints that have
// nothing to do with proposals. The damage is invisible in the default theme and
// obvious in `ink`: those surfaces ignore the user's theme entirely, because a
// theme-independent token is exactly what they asked for by using it.
//
// # Rule 2 - a solid brand FILL is not `--t-accent`
// `--t-accent` inverts by design: #7C3AED in lilac/blush, #DCCFFB in `ink`, so it
// stays legible as TEXT on that theme's dark surfaces. Put it behind white text
// and `ink` renders white on #DCCFFB, which is roughly 1.3:1 - invisible. Fills
// that carry white content use `--btn-primary-bg`, which `ink` overrides back to
// the dark violet precisely so this cannot happen.
//
// Neither rule produces an error, a failed build or a visual diff in the theme
// most people develop in. A test is the only thing that notices.

import { describe, expect, test } from 'bun:test'
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join } from 'node:path'

const ROOT = join(import.meta.dir, '..')

function walk(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    if (name === 'node_modules' || name === '.next' || name === 'out') continue
    const p = join(dir, name)
    if (statSync(p).isDirectory()) walk(p, out)
    else if (/\.tsx?$/.test(name)) out.push(p)
  }
  return out
}

const SOURCES = [...walk(join(ROOT, 'app')), ...walk(join(ROOT, 'components'))]
const rel = (p: string) => p.slice(ROOT.length + 1)

// The one sanctioned use. WhatsNewModal's release pill is a FIXED brand chip
// carrying DARK text, documented as deliberately theme-independent at the call
// site - both candidate replacements would wreck its contrast.
const ALLOWED = new Set(['components/timeline/WhatsNewModal.tsx'])

describe('--color-state-proposal stays a state colour', () => {
  test('nothing outside the sanctioned exception uses it', () => {
    const offenders = SOURCES
      .filter((p) => readFileSync(p, 'utf8').includes('var(--color-state-proposal)'))
      .map(rel)
      .filter((p) => !ALLOWED.has(p))

    expect(offenders).toEqual([])
  })
})

describe('solid brand fills do not use --t-accent', () => {
  // Matches `background: … var(--t-accent) …` where the value is NOT a
  // color-mix tint. A tint is fine in either theme - it is a wash over the
  // surface beneath it, not a field that has to hold white text.
  const PROPS = /\b(background|backgroundColor|color|borderColor|borderTopColor|border|outline|boxShadow|stroke|fill)\s*[:=]/g

  test('no solid --t-accent background sits near white content', () => {
    const offenders: string[] = []
    for (const p of SOURCES) {
      const lines = readFileSync(p, 'utf8').split('\n')
      lines.forEach((line, i) => {
        for (const m of line.matchAll(/var\(--t-accent\)/g)) {
          const before = line.slice(0, m.index)
          const props = [...before.matchAll(PROPS)]
          const last = props[props.length - 1]
          if (!last) continue
          if (last[1] !== 'background' && last[1] !== 'backgroundColor') continue
          // A tint, not a fill.
          if (before.slice(last.index).includes('color-mix')) continue
          const window = lines.slice(Math.max(0, i - 5), i + 6).join('\n')
          if (/'#fff|"#fff|'white'/.test(window)) offenders.push(`${rel(p)}:${i + 1}`)
        }
      })
    }
    expect(offenders).toEqual([])
  })
})
