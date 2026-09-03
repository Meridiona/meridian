//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The setup wizard is set in ONE typeface, and it is the app's.
//
// `--font-sans` is `-apple-system, BlinkMacSystemFont, 'SF Pro Text', system-ui,
// sans-serif` (globals.css), i.e. SF Pro on macOS - and `--font-mono` is an
// alias of it, so both tokens land on the same face. Everything in the wizard
// resolves through one of the two, or inherits from `body`.
//
// This is guarded rather than assumed because the wizard has drifted twice: the
// headings were set in Instrument Serif until they were moved onto
// `var(--font-sans)`, and two `<code>` elements on the sign-in step were still
// hard-coded to the browser's `monospace` after that - a third face on a screen
// meant to have one, in the first thing a new user ever sees.
//
// A hard-coded family is the specific thing being caught. The tokens are fine;
// a literal `'monospace'` / `serif` / a named font is not, because it cannot
// follow the app if the stack ever changes.

import { describe, it, expect } from 'bun:test'
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join } from 'node:path'

const setupRoot = join(import.meta.dir, '..', 'app', 'setup')

/** Every source file under `app/setup/`, discovered rather than listed - a step
 *  file added later is covered the day it lands. */
function walk(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    const full = join(dir, name)
    if (statSync(full).isDirectory()) walk(full, out)
    else if (/\.tsx?$/.test(name)) out.push(full)
  }
  return out
}

/** `src` with comments removed.
 *
 *  Not optional. The first version of this file matched "Instrument Serif" in
 *  `atoms.tsx`'s own header - a comment RECORDING that the headings were moved
 *  OFF it. A source scan that reads prose as code either fails on correct code
 *  (here) or, more often, passes because the needle it wants is sitting in a
 *  comment; both make the guard worthless. Strings are left intact, since a
 *  hard-coded family lives in one. */
function code(src: string): string {
  return src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/^\s*(\/\/|\*).*$/gm, '')
}

describe('the setup wizard uses one typeface', () => {
  const files = walk(setupRoot)

  it('scans the whole wizard', () => {
    // A walk that finds nothing passes every assertion below in silence.
    expect(files.length).toBeGreaterThan(4)
  })

  it('never hard-codes a font family', () => {
    // `fontFamily: 'inherit'` is allowed and is not a family - it is the opposite,
    // an explicit refusal to pick one (OtpForm's input uses it so the OTP
    // boxes match the surrounding card).
    const offenders: string[] = []
    for (const f of files) {
      code(readFileSync(f, 'utf8')).split('\n').forEach((line, i) => {
        const m = line.match(/fontFamily:\s*'([^']+)'/)
        if (!m) return
        const value = m[1]
        if (value.startsWith('var(--font-') || value === 'inherit') return
        offenders.push(`${f.slice(setupRoot.length + 1)}:${i + 1}  ${value}`)
      })
    }
    expect(offenders).toEqual([])
  })

  it('never reaches for a serif', () => {
    // The wizard's headings were Instrument Serif once. The timeline UI has one
    // voice and the wizard is the first screen of it.
    for (const f of files) {
      const src = code(readFileSync(f, 'utf8'))
      expect(src, `${f} names a serif`).not.toMatch(/font-serif|Instrument Serif|Georgia/)
    }
  })
})
