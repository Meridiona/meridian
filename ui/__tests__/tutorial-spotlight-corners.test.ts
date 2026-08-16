//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The walkthrough's dimmed beat must not show square corners.
//
// `Cutout` fences the spotlight with four plain rectangles rather than one
// masked pane, because `backdrop-filter` under a mask is exactly the
// combination WebKit is unreliable about and this ships inside a WKWebView.
// The consequence is a hole with square corners, and for a long time the file
// claimed that did not matter because "the spotlight ring drawn on top is
// rounded and reads as the frame".
//
// It did matter. The ring sits `RING_INSET` outside the target; the hole sits
// `FENCE_PAD` outside it. `FENCE_PAD` was the larger of the two, so the hole
// was not under the ring at all - it stuck out past it, and each corner showed
// as a sharp un-dimmed nub around a rounded card.
//
// Source-scanning: this is a compositing artifact across stacked
// fixed-position layers with a backdrop filter. A headless run has no
// backdrop to filter and no corner to rasterise, so there is nothing for a
// DOM assertion to look at. What IS checkable is the geometry the layers are
// given, which is where the bug lived.

import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'

const overlay = readFileSync(
  join(import.meta.dir, '..', 'components', 'tutorial', 'TutorialOverlay.tsx'), 'utf8',
)

/** Reads a module-level `const NAME = <arithmetic>` out of the source and works
 *  out its value, so a constant defined in terms of the others (`HOLE_RADIUS`)
 *  is read as the number it actually resolves to rather than skipped.
 *
 *  Returns NaN for anything missing or not plain arithmetic, which fails the
 *  comparisons below rather than passing vacuously. */
function num(name: string, seen: string[] = []): number {
  if (seen.includes(name)) return NaN // a cycle - refuse rather than recurse
  const m = overlay.match(new RegExp(`^const ${name} = ([^\\n]+)`, 'm'))
  if (!m) return NaN
  const rhs = m[1].trim()
  // Substitute any constant it refers to, then check what is left is only
  // arithmetic before evaluating it.
  const resolved = rhs.replace(/[A-Z_][A-Z0-9_]*/g, ref => String(num(ref, [...seen, name])))
  if (!/^[-+*/(). \d]+$/.test(resolved) || resolved.includes('NaN')) return NaN
  return Number(new Function(`return (${resolved})`)())
}

describe('the walkthrough spotlight has no square corners', () => {
  it('draws the dim on a rounded element, not on the square fence panes', () => {
    // The four panes stay square - that is the whole point of building the
    // fence out of rectangles. What must NOT be square is the thing the eye
    // actually reads as the edge of the lit area: the dim.
    //
    // A scrim with a very large `box-shadow` spread is the one construction
    // that gives a rounded hole without a mask: box-shadow honours
    // `border-radius`, and its overflow is ink rather than layout, so a spread
    // far past the viewport costs nothing.
    const at = overlay.indexOf('function Cutout({ rect, dim }')
    expect(at).toBeGreaterThan(-1)
    const cutout = overlay.slice(at)

    const dimColour = cutout.indexOf('rgba(20,16,40,0.42)')
    expect(dimColour).toBeGreaterThan(-1)
    // The dim must be carried by a declaration that also rounds its corners.
    // Before the fix it sat in the panes' style object, which has no radius and
    // never could have one.
    const decl = cutout.slice(Math.max(0, dimColour - 400), dimColour + 400)
    expect(decl).toContain('boxShadow')
    expect(decl).toContain('borderRadius')
  })

  it('keeps the blur on the panes, so the fence stays one component', () => {
    // Only the dim moved. The blur, the pointer fence and the geometry are
    // still the same four panes for both variants - splitting them would let
    // the invisible fence drift out of step with the visible one silently.
    const at = overlay.indexOf('function Cutout({ rect, dim }')
    const cutout = overlay.slice(at)
    expect(cutout).toContain('backdropFilter')
    expect(overlay).not.toContain('function DimCutout')
  })

  it('rounds the dim hole at least as much as the ring it encloses', () => {
    // The invariant, stated as the parallel-curve condition: subtracting each
    // shape's own outward offset leaves the radius it would have if it were
    // drawn on the target itself. The hole's must not be tighter than the
    // ring's, or the hole's corners cut inside the ring and reappear.
    //
    // This is what failed before: FENCE_PAD 8 against a ring at inset 6 with
    // radius 24 and no radius of its own at all - a corner sitting ~13px
    // outside the ring's arc.
    const ringInset = num('RING_INSET')
    const ringRadius = num('RING_RADIUS')
    const fencePad = num('FENCE_PAD')
    const holeRadius = num('HOLE_RADIUS')

    for (const v of [ringInset, ringRadius, fencePad, holeRadius]) {
      expect(Number.isFinite(v)).toBe(true)
    }
    expect(holeRadius - fencePad).toBeGreaterThanOrEqual(ringRadius - ringInset)
  })

  it('feeds those constants to the ring and the fence rather than re-typing them', () => {
    // Two copies of the same number is how this drifts back: someone nudges
    // the ring's radius, the hole keeps the old one, and the corners return
    // with nothing failing. The constants are only worth having if both
    // shapes are actually drawn from them.
    expect(overlay).toMatch(/borderRadius: RING_RADIUS\b/)
    expect(overlay).toMatch(/borderRadius: HOLE_RADIUS\b/)
    expect(overlay).toMatch(/ringBox\.left - RING_INSET/)
    expect(overlay).toMatch(/rect\.left - FENCE_PAD/)
  })

  it('moves the dim with the hole instead of snapping it', () => {
    // The panes glide between beats. A scrim that re-lays instantly would make
    // the dim hole jump while the blur slides after it - a worse artifact than
    // the square corner this replaced. Same easing, same duration.
    const at = overlay.indexOf('function Cutout({ rect, dim }')
    const cutout = overlay.slice(at)
    // Asserted as SHARING, not as two matching copies: two copies is the thing
    // that drifts. One value, applied to both.
    expect(cutout).toMatch(/const glide = 'left \.5s cubic-bezier\(\.22,1,\.32,1\)/)
    // At least the panes and the scrim. Not an exact count - a third layer that
    // legitimately glides would fail a correct file, and the guarantee this
    // test exists for (both shapes read one value) does not depend on how many
    // things read it.
    const uses = cutout.match(/transition: glide\b/g) ?? []
    expect(uses.length).toBeGreaterThanOrEqual(2)
    expect(cutout).toContain("animation: 'mer-tour-dim .45s ease both'")
  })
})
