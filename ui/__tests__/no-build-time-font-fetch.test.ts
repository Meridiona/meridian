//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// `next/font/google` must not be used: it makes every `next build` depend on
// fonts.gstatic.com being reachable AND serving the exact file hashes Next
// resolved.
//
// THIS BROKE CI, not hypothetically. On 2026-08-16 the UI job failed with six
// `Received response with status 404 when requesting
// https://fonts.gstatic.com/s/jetbrainsmono/v24/...woff2` errors and
// `Turbopack build failed with 12 errors`. Nothing in the repo had changed and
// the same commit built fine locally - Google had rotated the v24 file hashes,
// so a machine with a warm font cache never noticed while a cold CI runner
// could not resolve a single file. No re-run would have fixed it, and the same
// failure would have hit the release job.
//
// The failure mode is the problem as much as the outage: a third party can turn
// the build red at any moment, for a font used by one row of timestamp labels.
//
// Vendor the file and use `next/font/local` instead (see `app/layout.tsx`).
// That is a one-line API difference and removes the network from the build
// entirely - verified by building with the network blackholed, which fails on
// the loader and passes on the vendored file.

import { describe, expect, it } from 'bun:test'
import { readdirSync, readFileSync, statSync } from 'fs'
import { join } from 'path'

const uiRoot = join(import.meta.dir, '..')

// Directories that are NOT shipped source, and why each is skipped. Everything
// else under `ui/` is scanned.
//
// DISCOVERED RATHER THAN LISTED. This used to name the three source directories
// that exist today (`app`, `components`, `lib`), which meant a fourth one added
// later was silently outside the rule - and the failure it guards against
// (`next build` fetching a font from Google, 404ing on cold CI, taking the whole
// build with it) is one nobody would connect back to an unscanned directory.
// Inverting it means new source is covered the day it lands.
const NOT_SHIPPED_SOURCE = new Set([
  'node_modules',
  '.next', // build output
  'out', // static export
  'public', // assets, not compiled source
  '__tests__', // this file mentions the loader by name; it must not trip its own rule
])

/** Every compiled source file under `dir`. Includes `.js`/`.mjs` as well as
 *  TypeScript: a plain-JS module is just as capable of importing the loader,
 *  and `postcss.config.mjs` sits at this root today. */
function walk(dir: string, out: string[] = []): string[] {
  for (const entry of readdirSync(dir)) {
    if (NOT_SHIPPED_SOURCE.has(entry) || entry.startsWith('.')) continue
    const full = join(dir, entry)
    if (statSync(full).isDirectory()) walk(full, out)
    else if (/\.(ts|tsx|js|jsx|mjs|cjs)$/.test(entry)) out.push(full)
  }
  return out
}

describe('the production build never fetches fonts', () => {
  it('does not import next/font/google anywhere', () => {
    const scanned = walk(uiRoot)
    // The scan finding nothing to scan would pass silently, which is the usual
    // way a source-scan stops testing anything without anyone noticing.
    expect(scanned.length).toBeGreaterThan(50)
    const offenders = scanned
      .filter(f => /from ['"]next\/font\/google['"]/.test(readFileSync(f, 'utf8')))
      .map(f => f.slice(uiRoot.length + 1))
    expect(offenders).toEqual([])
  })

  it('loads JetBrains Mono from a vendored file', () => {
    const layout = readFileSync(join(uiRoot, 'app', 'layout.tsx'), 'utf8')
    expect(layout).toContain("import localFont from 'next/font/local'")
    expect(layout).toContain("src: './fonts/JetBrainsMono-latin.woff2'")
  })

  it('ships the file the loader points at, and its licence', () => {
    // A missing file fails the build loudly, but the LICENCE going missing
    // would not - and the OFL requires the notice to travel with the font.
    const fonts = readdirSync(join(uiRoot, 'app', 'fonts'))
    expect(fonts).toContain('JetBrainsMono-latin.woff2')
    expect(fonts).toContain('OFL.txt')
    const licence = readFileSync(join(uiRoot, 'app', 'fonts', 'OFL.txt'), 'utf8')
    expect(licence).toContain('SIL Open Font License')
  })
})
