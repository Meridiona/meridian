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

// Source we ship. `__tests__` is excluded so this file's own mention of the
// loader does not trip its own rule.
const SCANNED = ['app', 'components', 'lib']

function walk(dir: string, out: string[] = []): string[] {
  for (const entry of readdirSync(dir)) {
    if (entry === 'node_modules' || entry === '.next' || entry === 'out') continue
    const full = join(dir, entry)
    if (statSync(full).isDirectory()) walk(full, out)
    else if (/\.(ts|tsx)$/.test(entry)) out.push(full)
  }
  return out
}

describe('the production build never fetches fonts', () => {
  it('does not import next/font/google anywhere', () => {
    const offenders: string[] = []
    for (const root of SCANNED) {
      for (const file of walk(join(uiRoot, root))) {
        if (/from ['"]next\/font\/google['"]/.test(readFileSync(file, 'utf8'))) {
          offenders.push(file.slice(uiRoot.length + 1))
        }
      }
    }
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
