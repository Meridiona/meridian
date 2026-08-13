//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
// Guards the consumer half of the `deep_link` contract.
//
// A notification's deep link crosses a language boundary: Rust producers stamp
// a string onto an outbox row, and `MeridianTimelineShell.tsx`'s `navigate`
// turns it into a modal or a settings section. Nothing type-checks that hop.
// When the Next fold deleted the `/tasks` route, `navigate` quietly stopped
// resolving `/tasks?integrations=1` while all five `pm.*` sync faults kept
// emitting it — no error, no log, no failing test, just a [View] button that
// opened the default view for months.
//
// So the vocabulary is declared once, in Rust
// (`meridian_core::notifications::deep_links`), and checked from both ends:
//   - `tests/deep_links.rs` — no producer emits a value outside the vocabulary.
//   - this file           — `navigate` resolves every value in the vocabulary.
// Either alone passes vacuously: producers could all agree on a link the shell
// ignores, or the shell could handle links nobody sends.
import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'fs'

const repoRoot = import.meta.dir + '/../..'
const read = (rel: string): string => readFileSync(repoRoot + '/' + rel, 'utf8')

const CONSTANTS = 'meridian-core/src/notifications/mod.rs'
const SHELL = 'ui/components/timeline/MeridianTimelineShell.tsx'

// Pull `pub const NAME: &str = "value";` out of the `deep_links` module, and
// the `LEGACY` array's entries. Parsing the Rust rather than restating the
// values here is the whole point - a copy would drift silently, which is the
// class of bug this guards.
/** Slice from `start` to the brace that closes the block opening at `start`. */
function braceBlock(src: string, start: number): string {
  const open = src.indexOf('{', start)
  let depth = 0
  for (let i = open; i < src.length; i++) {
    if (src[i] === '{') depth++
    else if (src[i] === '}' && --depth === 0) return src.slice(start, i + 1)
  }
  return src.slice(start)
}

function parseDeepLinks(src: string): { all: string[]; legacy: string[] } {
  const modStart = src.indexOf('pub mod deep_links {')
  expect(modStart).toBeGreaterThan(-1)
  // Brace-matched, NOT sliced to EOF. Slicing to the end of the file worked
  // only because `deep_links` happens to be the last thing declaring a
  // `pub const X: &str`; one unrelated const below it would be absorbed into
  // `all`, and this suite would then demand a navigate() arm for it and fail
  // pointing at the wrong thing entirely.
  const body = braceBlock(src, modStart)

  const all: string[] = []
  for (const m of body.matchAll(/pub const [A-Z_]+: &str = "([^"]+)";/g)) all.push(m[1])

  const legacyDecl = body.match(/pub const LEGACY: \[&str; \d+\] = \[([\s\S]*?)\];/)
  expect(legacyDecl).not.toBeNull()
  const legacy = [...legacyDecl![1].matchAll(/"([^"]+)"/g)].map(m => m[1])

  // The scalar consts include the LEGACY strings only if they were declared as
  // consts, which they are not - LEGACY is an inline array. So `all` is exactly
  // the current vocabulary.
  return { all, legacy }
}

describe('deep-link vocabulary', () => {
  const { all, legacy } = parseDeepLinks(read(CONSTANTS))
  const shell = read(SHELL)

  it('parses a non-empty vocabulary out of the Rust constants', () => {
    // A parser that silently matched nothing would make every assertion below
    // pass over an empty list.
    expect(all.length).toBeGreaterThanOrEqual(7)
    expect(legacy.length).toBeGreaterThanOrEqual(2)
    expect(all).toContain('/plan')
    expect(all).toContain('/settings/integrations')
  })

  it('navigate() handles every current deep link', () => {
    for (const link of all) {
      expect(shell.includes(`case '${link}':`)).toBe(true)
    }
  })

  it('navigate() still handles the retired spellings', () => {
    // Undelivered rows and older installs carry these. A producer change does
    // not rewrite rows already in the outbox - the same lesson as the 22 dead
    // board-hygiene banners that needed their own migration.
    for (const link of legacy) {
      expect(shell.includes(`case '${link}':`)).toBe(true)
    }
  })

  it('navigate() has a default arm, so an unknown target is never silent', () => {
    // The absence of this is the root cause of the original bug: a target that
    // matched nothing did nothing, indistinguishably from success.
    //
    // Bounded to navigate's own body. Slicing to EOF would be satisfied by a
    // `default:` or `console.warn` anywhere in the remaining ~300 lines of the
    // shell - passing by coincidence of file ordering, which is the failure
    // mode this whole suite exists to rule out.
    const nav = braceBlock(shell, shell.indexOf('const navigate ='))
    expect(nav.length).toBeLessThan(3000) // it really is bounded
    expect(nav).toContain('default:')
    expect(nav).toContain('console.warn')
  })
})
