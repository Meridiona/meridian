//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The DMG background has to tell the user to OPEN the app, not only to drag it.
//
// A disk image cannot launch anything. The drag to Applications is a Finder
// copy - no code of ours runs, and macOS has had no post-install hook in a DMG
// by design for as long as Gatekeeper has existed. So an app sitting in
// Applications unopened after an install is not something the app can fix from
// the inside: it is a step the installer has to ASK for, and the background
// image is the only surface in a DMG that can ask.
//
// It used to say "Drag to Applications to install" and stop. That describes the
// drag accurately and then leaves the user on a Finder window with nothing
// indicating the app has not started - reported as "on dropping Meridian to
// Applications can we make the app setup open immediately?", which is the same
// observation from the other side.
//
// Guarded here because the failure is silent in the worst way: the background
// is a generated PNG, so nothing type-checks it, nothing renders it in CI, and
// a well-meaning copy edit that collapses the two steps back into one sentence
// looks completely fine in the diff.
//
// This lives under ui/__tests__ because that is the only JS/TS suite the repo
// runs; the subject is a Python generator, so it is scanned as text.

import { describe, it, expect } from 'bun:test'
import { readFileSync, statSync } from 'fs'
import { inflateSync } from 'zlib'
import { join } from 'path'

const repo = join(import.meta.dir, '..', '..')
const gen = readFileSync(join(repo, 'scripts', 'make-dmg-background.py'), 'utf8')

/** The step strings the generator draws, in the order it draws them. */
function steps(): string[] {
  const at = gen.indexOf('    steps = [')
  expect(at).toBeGreaterThan(-1)
  const end = gen.indexOf('    ]', at)
  expect(end).toBeGreaterThan(at)
  return [...gen.slice(at, end).matchAll(/"([^"]+)"/g)].map(m => m[1])
}

/** Where the generator draws the step lines, read out of the generator itself.
 *
 *  `top` and `pitch` come from the one `draw.text(...)` offset expression, and
 *  `scale` from the `SCALE` constant the generator multiplies it by - so a
 *  render at a different resolution moves the band with it instead of leaving
 *  these tests measuring empty pixels. Parsed once, with a message per value:
 *  a non-null assertion on a reformatted generator throws a bare `TypeError`
 *  that says nothing about what changed. */
function captionGeometry(): { top: number; pitch: number; scale: number } {
  const offset = gen.match(/\((\d+) \+ n \* (\d+)\) \* SCALE/)
  expect(offset, 'the generator no longer draws the steps at `(top + n * pitch) * SCALE`').not.toBeNull()
  const scale = gen.match(/^SCALE = (\d+)/m)
  expect(scale, 'the generator no longer declares `SCALE` at the top level').not.toBeNull()
  return { top: Number(offset![1]), pitch: Number(offset![2]), scale: Number(scale![1]) }
}

/** Read a big-endian u32, since a plain `Uint8Array` has no `readUInt32BE`. */
function be32(b: Uint8Array, at: number): number {
  return ((b[at] << 24) | (b[at + 1] << 16) | (b[at + 2] << 8) | b[at + 3]) >>> 0
}

/** Decode the background PNG to greyscale rows.
 *
 *  Hand-rolled rather than pulled from a package: `ui/` ships in the tray
 *  binary, and adding an image library to its dependency tree to read one
 *  committed asset in one test is a poor trade. The scope is exactly this file
 *  - 8-bit truecolour, one IDAT, which is what `make-dmg-background.py` emits
 *  (asserted below, so a format change fails loudly instead of decoding to
 *  garbage). */
function decodeGrayRows(png: Uint8Array): { width: number; rows: number[][] } {
  let off = 8 // skip the signature
  let width = 0
  let height = 0
  const idat: Uint8Array[] = []
  while (off < png.length) {
    const len = be32(png, off)
    const type = String.fromCharCode(...png.subarray(off + 4, off + 8))
    if (type === 'IHDR') {
      width = be32(png, off + 8)
      height = be32(png, off + 12)
      expect(png[off + 16]).toBe(8) // bit depth
      expect(png[off + 17]).toBe(2) // colour type: truecolour, no alpha
      expect(png[off + 20]).toBe(0) // interlace: none
    }
    if (type === 'IDAT') idat.push(png.subarray(off + 8, off + 8 + len))
    off += 12 + len
  }
  expect(width).toBeGreaterThan(0)

  // Concatenated by hand rather than with `Buffer.concat`: with bun-types in
  // scope node's `Buffer` no longer satisfies zlib's `InputType` (its backing
  // store widens to `ArrayBufferLike`, which admits SharedArrayBuffer).
  const joined = new Uint8Array(idat.reduce((n, c) => n + c.length, 0))
  let at = 0
  for (const chunk of idat) { joined.set(chunk, at); at += chunk.length }
  const raw = inflateSync(joined)
  const bpp = 3
  const stride = width * bpp
  const out: number[][] = []
  // Undo the per-scanline filter. All five types are handled because Pillow
  // chooses per row and the choice is not ours to predict.
  const prev = new Uint8Array(stride)
  const cur = new Uint8Array(stride)
  for (let y = 0; y < height; y++) {
    const filter = raw[y * (stride + 1)]
    const line = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1))
    for (let i = 0; i < stride; i++) {
      const a = i >= bpp ? cur[i - bpp] : 0
      const b = prev[i]
      const c = i >= bpp ? prev[i - bpp] : 0
      let v = line[i]
      if (filter === 1) v += a
      else if (filter === 2) v += b
      else if (filter === 3) v += (a + b) >> 1
      else if (filter === 4) {
        const p = a + b - c
        const pa = Math.abs(p - a), pb = Math.abs(p - b), pc = Math.abs(p - c)
        v += pa <= pb && pa <= pc ? a : pb <= pc ? b : c
      } else if (filter !== 0) {
        // Same rule as the IHDR assertions: fail loudly rather than decode to
        // garbage. Without this an unknown byte falls through every branch and
        // is silently treated as filter 0, so the rows are nonsense and the
        // line count is an arbitrary number rather than an error.
        throw new Error(`unknown PNG scanline filter ${filter} on row ${y}`)
      }
      cur[i] = v & 0xff
    }
    const row: number[] = new Array(width)
    for (let x = 0; x < width; x++) {
      // Rec. 601 luma, enough to separate dark glyphs from a pale backdrop.
      row[x] = (cur[x * 3] * 299 + cur[x * 3 + 1] * 587 + cur[x * 3 + 2] * 114) / 1000
    }
    out.push(row)
    prev.set(cur)
  }
  return { width, rows: out }
}

/** How many separate lines of text are drawn in `rows`.
 *
 *  A row with any dark pixel is inked; a run of inked rows is one line. The
 *  threshold sits well below the backdrop (which is a pale lilac gradient,
 *  luma > 220) and well above the caption's own colour (`--t-muted`, luma ~110),
 *  so the gap between them is wide and the count is not sensitive to it. */
function countTextLines(rows: number[][], width: number): number {
  let lines = 0
  let inRun = false
  for (const row of rows) {
    let inked = false
    for (let x = 0; x < width; x++) {
      if (row[x] < 160) { inked = true; break }
    }
    if (inked && !inRun) lines++
    inRun = inked
  }
  return lines
}

describe('the DMG tells the user to open the app', () => {
  it('draws two steps, not one instruction', () => {
    // Numbered rather than one sentence on purpose: "and then open it" appended
    // to the drag instruction reads as a description of what the drag does,
    // which is exactly the misreading that leaves the app unopened.
    //
    // TWO is a product decision, not an incidental count: a DMG window is read
    // in a second and a half, and a third instruction is where people stop
    // reading any of them. So adding one should have to come here and argue for
    // itself. That is why this assertion is fixed while the pixel check below
    // derives its count from the generator - they are answering different
    // questions ("should there be three?" vs "is what we ship what we wrote?"),
    // and only the first is a judgement call.
    expect(steps()).toHaveLength(2)
  })

  it('names the drag first and the open second', () => {
    const [one, two] = steps()
    expect(one).toMatch(/^1\./)
    expect(one.toLowerCase()).toContain('drag')
    expect(two).toMatch(/^2\./)
    expect(two.toLowerCase()).toContain('open')
  })

  it('says WHERE to open it from', () => {
    // The user is looking at a DMG window, not at Applications - the folder is
    // the part that is not obvious at that moment.
    expect(steps()[1]).toContain('Applications')
  })

  it('uses plain hyphens only, like every other user-facing string', () => {
    // CLAUDE.md's hard rule. This text is read by a user during install, so it
    // is app text even though it lives in a build script.
    for (const s of steps()) {
      expect(s).not.toMatch(/[—–]|--/)
    }
  })

  it('keeps the steps clear of the icon row Finder draws on top', () => {
    // Finder paints the .app and the Applications alias at y=170pt at 128pt
    // (tauri.conf.json's bundle.macOS.dmg), so their bottom edge is 234pt and
    // their labels sit below that. Text placed any higher would be drawn under
    // an icon - invisible, with nothing in the build to notice.
    expect(captionGeometry().top).toBeGreaterThan(234)
  })

  it('ships a PNG with BOTH lines actually drawn in it', () => {
    // THE COMMITTED IMAGE IS THE ARTEFACT, and it is generated BY HAND - no
    // build step regenerates it. So editing the copy above and forgetting to
    // re-run the script ships the OLD picture while the diff looks complete.
    // Everything above this point reads the generator's SOURCE, so all of it
    // passes happily against a stale PNG. This is the assertion that looks at
    // what users are actually shipped.
    //
    // Two earlier attempts are recorded because each failed in a way that is
    // invisible unless you go looking:
    //
    //   1. Comparing mtimes. Unsound: on a fresh clone git gives every file the
    //      same checkout time, so it passes unconditionally in CI - the one
    //      place it needed to work. Measured, not assumed - with equal mtimes it
    //      passed against a deliberately stale PNG.
    //   2. Pinning a `rendered-digest` comment in the generator. Better, but the
    //      digest is hand-maintained, so the same edit that forgets to re-render
    //      can paste the OLD hash and the test still passes. (CodeRabbit's
    //      catch on #790.)
    //
    // Re-running the generator inside the test - the obvious remaining option -
    // is not available: it hard-exits without `/System/Library/Fonts/SFNS.ttf`,
    // and this suite runs on `ubuntu-latest`.
    //
    // So the PNG is measured directly. Rasterised text cannot be grepped, but
    // it can be COUNTED: scan the caption band for rows containing dark pixels
    // and count the runs of them. One run per line of text. The old one-line
    // caption gives one, which is exactly the state being guarded, and no
    // hand-maintained value stands between the check and the artefact.
    const png = new Uint8Array(readFileSync(join(repo, 'tray', 'src-tauri', 'dmg', 'background.png')))
    const { width, rows } = decodeGrayRows(png)

    // The band and the expected count both come FROM the generator, so adding a
    // third step and re-rendering correctly still passes. Hard-coding `2` would
    // fail on a correct change and point at the artefact rather than at the
    // stale expectation - which is the wrong place to send the reader.
    const { top, pitch, scale } = captionGeometry()
    const expected = steps().length
    // The band runs one FULL PITCH past the last line's baseline, not to it -
    // so a third step, drawn one pitch below the second, falls inside the band
    // and is counted. Ending at the last line would clip an added one, and a
    // clipped line reads as "not drawn", which is the same failure as a stale
    // PNG with a completely different cause.
    const band = rows.slice(top * scale - 8, (top + pitch * expected) * scale + 8)
    expect(countTextLines(band, width)).toBe(expected)
  })

  it('is a real image, not a placeholder or an LFS pointer', () => {
    // Cheap sanity on the artefact itself: PNG magic, and a size consistent with
    // a 1320x800 render rather than a stub committed by accident.
    const png = readFileSync(join(repo, 'tray', 'src-tauri', 'dmg', 'background.png'))
    expect([...png.subarray(0, 4)]).toEqual([0x89, 0x50, 0x4e, 0x47])
    expect(statSync(join(repo, 'tray', 'src-tauri', 'dmg', 'background.png')).size)
      .toBeGreaterThan(10_000)
  })
})
