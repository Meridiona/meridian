//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'fs'

// The walkthrough's un-anchored narration bar floats over the page at a fixed
// height, chosen to clear a modal's title row and ×. It does clear those - and
// then lands on the first line of the modal BODY, which starts only 28px lower.
// The AI-engine step's "Do you have a Claude, ChatGPT or Cursor subscription?"
// was rendered underneath it and simply could not be read.
//
// The bar cannot solve this by moving. It and the panel are both centred with
// overlapping max-widths, so they collide by construction; and the two modals
// with un-anchored beats (the AI gate, and the daily plan - script.ts's
// `s.say` calls with no following `s.spotlight`) are `scrollInside` shells that
// can reach `maxHeight: 92%`, leaving no room above or below to move it into.
//
// So the contract is a handshake, and it only works if BOTH halves are present:
// TutorialOverlay publishes where its bar ends, ModalShell pads its body past
// that. Either half alone is a silent no-op - the CSS var falls back and the
// heading goes back under the bar - which is what these tests exist to catch.
//
// Scans the two source files rather than rendering: the bug is a px collision
// between a fixed-position overlay and a modal's internal layout, which a
// headless DOM with no layout engine cannot reproduce at all.

const uiRoot = import.meta.dir + '/..'
const readSrc = (rel: string): string => readFileSync(uiRoot + '/' + rel, 'utf8')

const VAR = '--mer-tour-say-bottom'

describe("the tour's narration bar does not cover a modal's body", () => {
  it('TutorialOverlay publishes where the bar ends', () => {
    const src = readSrc('components/tutorial/TutorialOverlay.tsx')
    expect(src).toContain(`setProperty('${VAR}'`)
    // Measured from the live element. A constant would be wrong for most beats:
    // the bar runs one to three lines depending on the caption, and `big`
    // changes its font and padding.
    expect(src).toMatch(/sayRef\.current\?\.getBoundingClientRect\(\)/)
    expect(src).toContain('<div ref={sayRef}')
  })

  it('and withdraws it, so a stale value cannot outlive the bar', () => {
    const src = readSrc('components/tutorial/TutorialOverlay.tsx')
    // Twice: the not-showing branch, and the effect cleanup that covers unmount
    // and every caption change. A published value that survives the tour would
    // leave every modal in the app permanently padded.
    const removals = src.split(`removeProperty('${VAR}')`).length - 1
    expect(removals).toBeGreaterThanOrEqual(2)
  })

  it('ModalShell pads BOTH body branches past it', () => {
    const src = readSrc('components/timeline/ModalShell.tsx')
    // `scrollInside` is not the exotic branch here - it is the one both
    // affected modals use, so padding only the plain branch would fix nothing
    // that is actually broken.
    expect(src).toContain("TOUR_BODY_PAD('0px')")
    expect(src).toContain("TOUR_BODY_PAD('1.75rem')")
    // The fallback is what keeps this inert outside the tour: absent var ->
    // max() picks the branch's normal padding and nothing moves.
    expect(src).toContain(`var(${VAR}, 0px)`)
    expect(src).toMatch(/max\(\$\{base\}/)
  })

  it('the plain branch keeps its horizontal and bottom padding', () => {
    // `p-7` had to be split to make room for the computed `paddingTop`. Dropping
    // the other three sides would jam every non-scrollInside modal's content
    // against its own edges - a much more visible regression than the one being
    // fixed, and one no tour beat is needed to see.
    const src = readSrc('components/timeline/ModalShell.tsx')
    expect(src).toContain('px-7 pb-7')
    expect(src).not.toMatch(/nice-scroll p-7/)
  })
})
