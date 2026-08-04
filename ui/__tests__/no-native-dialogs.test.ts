//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Guards the one bug class that cannot be caught by reading the code, running it
// in `next dev`, or checking the logs.
//
// `window.confirm()` / `alert()` / `prompt()` reach WKWebView, which routes JS
// dialogs through the host app's `WKUIDelegate`. Nothing in this stack installs
// one - `wry`, `tauri-runtime-wry`, `tauri` and `tauri-utils` were each grepped
// for `ConfirmPanel|AlertPanel|WKUIDelegate` and every one is empty - so the
// dialogs never appear and `confirm()` always returns `false`.
//
// The damage is that a falsy `confirm()` is indistinguishable from the user
// clicking Cancel. The gated action quietly never runs, no error is raised, and
// every log line, gate and test still passes. That is exactly how the
// `db.corrupt` Repair Database button shipped dead: its consent gate could never
// return true, and its `window.alert` error path was equally invisible, so the
// recovery had to be driven by hand from a terminal.
//
// Use `components/ConfirmDialog.tsx` instead.

import { describe, it, expect } from 'bun:test'
import { readdirSync, readFileSync, statSync } from 'fs'
import { join } from 'path'

const uiRoot = join(import.meta.dir, '..')

// Source we ship into the webview. `__tests__` is excluded on purpose - a test
// asserting on these names (this file) must not trip its own rule.
const SCANNED = ['app', 'components', 'lib']

const SKIP_DIRS = new Set(['node_modules', '.next', 'out'])

function sourceFiles(dir: string): string[] {
  const found: string[] = []
  for (const entry of readdirSync(dir)) {
    if (SKIP_DIRS.has(entry)) continue
    const full = join(dir, entry)
    if (statSync(full).isDirectory()) {
      found.push(...sourceFiles(full))
    } else if (/\.(ts|tsx)$/.test(entry)) {
      found.push(full)
    }
  }
  return found
}

// Matches `window.confirm(`, `confirm(`, `globalThis.alert(` and friends, while
// leaving member calls on other objects alone (`this.prompt(`, `x.confirm(`) and
// ignoring anything preceded by an identifier character (`onConfirm(`).
const BANNED = /(?<![.\w])(?:(?:window|globalThis|self)\.)?(?:confirm|alert|prompt)\s*\(/

// A local `function confirm() {}` shadows the global, so calls to it are safe.
// ReviewRejectPicker.tsx has one. Declarations only - `const confirm = () =>`
// would shadow too, but naming a local after a banned global is confusing
// enough that the rule is worth keeping tight rather than exhaustive.
const DECLARATION = /\bfunction\s+(?:confirm|alert|prompt)\s*\(/

// Blanks out comments AND string/template literals so prose about the ban does
// not trip it - including JSX `{/* … */}` blocks, whose continuation lines start
// with neither `//` nor `*`. Newlines are preserved so reported line numbers stay
// honest.
//
// One left-to-right pass rather than two regex replaces, because the two
// constructs can hide each other and a regex cannot see that: a `//` inside a
// string literal (`'https://…'`) used to blank the rest of that real line of
// code, so a `window.confirm(` after it on the same line would go unreported.
// That is a false NEGATIVE in the one guard standing between us and another
// silently-dead button - precisely the failure this file exists to prevent - so
// it is worth the extra dozen lines. Walking left to right also stops the
// inverse error, where an apostrophe inside a comment ("don't") would otherwise
// open a bogus string literal and blank the code that follows.
function stripCommentsAndStrings(src: string): string {
  let out = ''
  let i = 0
  const blank = (c: string) => (c === '\n' ? '\n' : ' ')
  while (i < src.length) {
    const two = src.slice(i, i + 2)
    if (two === '//') {
      while (i < src.length && src[i] !== '\n') out += ' ', i++
      continue
    }
    if (two === '/*') {
      const end = src.indexOf('*/', i + 2)
      const stop = end === -1 ? src.length : end + 2
      for (; i < stop; i++) out += blank(src[i])
      continue
    }
    const q = src[i]
    if (q === '"' || q === "'" || q === '`') {
      out += ' '
      i++
      while (i < src.length) {
        if (src[i] === '\\') {
          out += '  '
          i += 2
          continue
        }
        if (src[i] === q) {
          out += ' '
          i++
          break
        }
        out += blank(src[i])
        i++
      }
      continue
    }
    out += src[i]
    i++
  }
  return out
}

describe('native browser dialogs are never used', () => {
  const files = SCANNED.flatMap((d) => sourceFiles(join(uiRoot, d)))

  it('finds source to scan (guards against a silently empty sweep)', () => {
    expect(files.length).toBeGreaterThan(50)
  })

  it('no window.confirm / alert / prompt anywhere in shipped UI source', () => {
    const offenders: string[] = []
    for (const file of files) {
      stripCommentsAndStrings(readFileSync(file, 'utf8'))
        .split('\n')
        .forEach((line, i) => {
          if (DECLARATION.test(line)) return
          if (BANNED.test(line)) {
            offenders.push(`${file.slice(uiRoot.length + 1)}:${i + 1}: ${line.trim()}`)
          }
        })
    }
    expect(offenders).toEqual([])
  })
})

// The sweep above is only as trustworthy as its stripper: every hole here is a
// silent false negative, which is the same failure mode as the dead button this
// file guards against. So the stripper is tested directly rather than trusted.
describe('the source stripper', () => {
  it('does not let a // inside a string hide a banned call on the same line', () => {
    // The regression. The previous two-regex stripper blanked from the `//` in
    // the URL to the end of the line, so the confirm() after it vanished and the
    // sweep reported the file clean.
    const src = `const help = 'https://meridiona.com/docs'; if (window.confirm('go')) run()`
    expect(BANNED.test(stripCommentsAndStrings(src))).toBe(true)
  })

  it('does not let an apostrophe in a comment blank the code after it', () => {
    // The inverse error: a naive string-blanking pass would treat the `'` in
    // "don't" as opening a literal and swallow the real call below.
    const src = ["// we don't ever want this", "window.confirm('really?')"].join('\n')
    expect(BANNED.test(stripCommentsAndStrings(src))).toBe(true)
  })

  it('still ignores prose about the banned globals', () => {
    expect(BANNED.test(stripCommentsAndStrings('// never call window.confirm(x)'))).toBe(false)
    expect(BANNED.test(stripCommentsAndStrings('/* alert( is banned */'))).toBe(false)
    expect(BANNED.test(stripCommentsAndStrings("const msg = 'use confirm(…) never'"))).toBe(false)
  })

  it('keeps line numbers honest so offender reports point at the real line', () => {
    const src = ['/* a\n   multi\n   line */', "const s = 'x\\ny'", 'window.alert(1)'].join('\n')
    const stripped = stripCommentsAndStrings(src)
    expect(stripped.split('\n').length).toBe(src.split('\n').length)
    expect(stripped.split('\n').findIndex((l) => BANNED.test(l))).toBe(4)
  })
})

describe('the replacement exists and is wired up', () => {
  it('ConfirmDialog is a real component', () => {
    const src = readFileSync(join(uiRoot, 'components/ConfirmDialog.tsx'), 'utf8')
    expect(src).toContain('export default function ConfirmDialog')
    // Escape must cancel - a modal with no keyboard exit is worse than none.
    expect(src).toContain("e.key === 'Escape'")
    // Errors render inside the dialog; window.alert cannot report them.
    expect(src).toContain('error')
  })

  it('the db.corrupt repair button gates on ConfirmDialog', () => {
    const src = readFileSync(join(uiRoot, 'components/NoticeBar.tsx'), 'utf8')
    expect(src).toContain('ConfirmDialog')
    expect(src).toContain('request_repair')
  })
})
