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

// Blanks out comments so prose about the ban does not trip it - including JSX
// `{/* … */}` blocks, whose continuation lines start with neither `//` nor `*`.
// Newlines are preserved so reported line numbers stay honest. Naive about `//`
// inside string literals (a URL truncates the rest of that line), which can only
// cause a false negative on a line that also calls a banned global - not a
// false positive, and the tighter parse is not worth a tokeniser here.
function stripComments(src: string): string {
  return src
    .replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\n]/g, ' '))
    .replace(/\/\/[^\n]*/g, '')
}

describe('native browser dialogs are never used', () => {
  const files = SCANNED.flatMap((d) => sourceFiles(join(uiRoot, d)))

  it('finds source to scan (guards against a silently empty sweep)', () => {
    expect(files.length).toBeGreaterThan(50)
  })

  it('no window.confirm / alert / prompt anywhere in shipped UI source', () => {
    const offenders: string[] = []
    for (const file of files) {
      stripComments(readFileSync(file, 'utf8'))
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
