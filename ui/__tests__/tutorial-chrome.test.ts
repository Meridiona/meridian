//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The walkthrough's CHROME - the surfaces the tour speaks from, as opposed to
// the product surfaces it points at.
//
// Ported from a Claude Design mockup (`Onboarding Tour.dc.html`) that reframed
// onboarding as a narrated story: full-screen takeover beats between the
// hands-on stretches, narration typed rather than printed, and a progress row
// so the user can see how much is left. What is NOT ported is the mockup's
// self-contained demo - Meridian's tour drives the real planner, the real
// composer and the real worklog dialog, and collects a plan, a tracker and an
// AI provider while it does. The chrome changed; what is underneath did not.
//
// Source-scanning, like every sibling walkthrough test: these are timing,
// pointer-routing and stacking properties across fixed-position layers. A
// headless run has no layout to measure and no frames to advance, and the
// failure modes here are all "it renders, but wrong" rather than an exception.

import { describe, it, expect } from 'bun:test'
import { readFileSync, existsSync } from 'node:fs'
import { join } from 'node:path'

const dir = (...p: string[]) => join(import.meta.dir, '..', ...p)
const read = (...p: string[]) => readFileSync(dir(...p), 'utf8')

const engine = read('components', 'tutorial', 'engine.ts')
const overlay = read('components', 'tutorial', 'TutorialOverlay.tsx')
const script = read('components', 'tutorial', 'script.ts')
const scriptDay = read('components', 'tutorial', 'scriptDay.ts')
const hook = read('components', 'tutorial', 'useTutorial.tsx')

const TYPEWRITER = dir('components', 'tutorial', 'useTypewriter.ts')
const TAKEOVER = dir('components', 'tutorial', 'TutorialTakeover.tsx')

describe('the tour can always be outpaced', () => {
  it('exposes a way to finish the typing instantly', () => {
    // A line typed at reading speed is a pace, not a gate. Someone re-running
    // the tour, or reading faster than 45ms a character, must be able to get
    // the whole line at once - otherwise the animation that exists to make the
    // narration readable is the thing making it unreadable.
    expect(existsSync(TYPEWRITER)).toBe(true)
    const tw = readFileSync(TYPEWRITER, 'utf8')
    expect(tw).toMatch(/finish/)
    // …and the takeover wires it to the surface, not just to a button: the
    // mockup lets you click anywhere on the card to land the line.
    expect(existsSync(TAKEOVER)).toBe(true)
    expect(readFileSync(TAKEOVER, 'utf8')).toMatch(/finish/)
  })

  it('keeps one typing implementation, not one per surface', () => {
    // Three surfaces type: the takeover hero, the anchored caption, and the
    // opening. Three implementations means three pacing constants, and the two
    // that are not being looked at drift silently.
    const tw = readFileSync(TYPEWRITER, 'utf8')
    expect(tw).toMatch(/export function useTypewriter/)
    for (const consumer of [overlay, readFileSync(TAKEOVER, 'utf8')]) {
      expect(consumer).toMatch(/useTypewriter/)
    }
    // No second setInterval-based typing loop anywhere in the tutorial chrome.
    expect(overlay).not.toMatch(/setInterval/)
  })

  it('does not let the typing resize the bubble it is typing into', () => {
    // Two things downstream measure the caption's height. The un-anchored bar
    // publishes its own bottom edge as `--mer-tour-say-bottom` so ModalShell can
    // pad its body clear of it (#775) - via a ResizeObserver, so a bar that grows
    // line by line as it types re-pads the modal in steps WHILE the user reads
    // the line doing it. And the anchored bubble is placed off its own height, so
    // it would creep upward as it typed.
    //
    // The fix is a hidden copy of the full line holding the box open from the
    // first frame. Checked here because neither symptom is visible in a headless
    // run: there is no layout, and no modal to re-pad.
    const body = overlay.slice(overlay.indexOf('function CaptionBody'))
    const sizer = body.indexOf("visibility: 'hidden' }}>{text}")
    const typed = body.indexOf('{shown}')
    expect(sizer).toBeGreaterThan(-1)
    // The reserved copy renders the WHOLE text, the visible one the slice - and
    // the visible layer is taken out of flow so it cannot add to the box.
    expect(typed).toBeGreaterThan(sizer)
    expect(body).toMatch(/position: 'absolute', inset: 0/)
  })

  it('stops typing when the tour is torn down', () => {
    // The interval outlives the component otherwise, and keeps calling setState
    // on something React has unmounted - which in a walkthrough the user just
    // skipped is a console full of warnings and a leaked timer per beat.
    const tw = readFileSync(TYPEWRITER, 'utf8')
    expect(tw).toMatch(/clearInterval|clearTimeout/)
    expect(tw).toMatch(/return \(\) =>/)
  })
})

describe('progress is shown without printing a promise', () => {
  it('derives the dots from the chapter list rather than a literal', () => {
    // Two numbers that must agree is how a progress row ends up showing six
    // dots for a seven-chapter tour, with nothing failing.
    expect(engine).toMatch(/chapter\(/)
    expect(overlay).toMatch(/CHAPTERS/)
    expect(overlay).not.toMatch(/length: 7|Array\(7\)/)
  })

  it('never prints "step N of M"', () => {
    // The mockup does ("Step 1 of 12"), and it is the one thing in it we do not
    // take. This tour BRANCHES - board vs solo, AI connected vs not, planner
    // opened vs not - so a printed denominator is wrong for some users. The
    // file already made this call once: INTRO_LINES dropped "about 2 minutes"
    // because a promise a first-run screen visibly breaks costs more trust than
    // the number ever bought, and tutorial-sample-day pins that. Dots say
    // "roughly this much left" without claiming a total.
    for (const src of [overlay, script, scriptDay]) {
      expect(src).not.toMatch(/[Ss]tep \$\{|of \$\{steps|Step \d+ of/)
    }
  })

  it('marks chapters at the boundaries the script already has', () => {
    // The section banners are the structure. Markers placed anywhere else are a
    // second, invisible structure that has to be kept in step with the first.
    expect(script).toMatch(/s\.chapter\(/)
    expect(scriptDay).toMatch(/s\.chapter\(/)
  })
})

describe('the takeover is a beat, not a component the script reaches around', () => {
  it('is a Stage primitive like every other beat', () => {
    // Beats are written against `Stage` so the script stays readable top to
    // bottom with no React in it. A takeover rendered by reaching into the hook
    // would be the one beat you cannot find by reading the script.
    expect(engine).toMatch(/takeover\(/)
    expect(hook).toMatch(/takeover:/)
  })

  it('does not leave a second full-screen typed card behind', () => {
    // The opening IS a takeover. Keeping `TutorialIntro` as well means two
    // components rendering the same thing, and they drift - which is exactly
    // how the tour ended up with two narration treatments before.
    expect(existsSync(dir('components', 'tutorial', 'TutorialIntro.tsx'))).toBe(false)
    expect(hook).not.toMatch(/TutorialIntro/)
  })

  it('uses the app\'s own type, not the mockup\'s retired faces', () => {
    // The mockup renders its heroes in Instrument Serif, with Plus Jakarta Sans
    // and JetBrains Mono around them. All three were deliberately retired:
    // globals.css says "There is deliberately no --font-serif" and layout.tsx
    // records the wizards dropping the serif for SF Pro. The takeover reads as
    // a different register through scale, colour and layout instead.
    // Comments stripped first: the file's header NAMES the retired faces in
    // order to explain why it does not use them, so a check against the raw text
    // fails on a correct file.
    const take = readFileSync(TAKEOVER, 'utf8')
      .replace(/^\s*\/\/.*$/gm, '')
      .replace(/\/\*[\s\S]*?\*\//g, '')
    expect(take).not.toMatch(/Instrument Serif|Jakarta|JetBrains/)
    expect(take).not.toMatch(/#[0-9A-Fa-f]{6}/)   // tokens, so it themes
  })
})

describe('the narration stays where it can be acted on', () => {
  it('keeps the caption anchored beside its target', () => {
    // The mockup moves narration to a fixed bar at the bottom of the window.
    // This layout exists because an instruction read in one place and acted on
    // in another is the failure it was built to avoid - the ring and the words
    // about it should be one object.
    expect(overlay).toMatch(/function AnchoredCaption/)
    expect(overlay).toMatch(/anchored/)
  })

  it('says what the ring wants when it is waiting on a click', () => {
    // The ring pulses and nothing explains why. `awaiting` already exists and
    // already drives that pulse and the click fence, so this is display-only -
    // but without it a waiting beat is indistinguishable from a stuck one.
    expect(overlay).toMatch(/awaiting && ring/)
    expect(overlay.toLowerCase()).toMatch(/click the highlighted/)
  })

  it('only offers that hint where there is actually something ringed', () => {
    // The hint lives in the ANCHORED bubble, which only renders for a beat with
    // a spotlight. That is what makes "click the highlighted card" impossible to
    // show when nothing is highlighted - and the script has FIVE beats that wait
    // on the user with the ring deliberately cleared (see #786: the tracker step,
    // the subscription question, the provider flow, Settings' close, the schedule
    // dialog). Each of those renders the un-anchored bar instead.
    const anchored = overlay.indexOf('<AnchoredCaption rect={ring}>')
    const topBar = overlay.indexOf('{unanchored && (')
    const hint = overlay.indexOf('<ClickHint />')
    expect(anchored).toBeGreaterThan(-1)
    expect(topBar).toBeGreaterThan(anchored)
    expect(hint).toBeGreaterThan(anchored)
    expect(hint).toBeLessThan(topBar)
    // Exactly one - a second copy in the top bar is the regression this forbids.
    expect(overlay.match(/<ClickHint \/>/g)).toHaveLength(1)
  })

  it('tells a click wait apart from a typing wait', () => {
    // "Click the highlighted card" over a field the user is meant to type more
    // words into is a confidently wrong instruction, which is worse than the
    // silence it replaced. The kind is carried from the primitive that set it
    // rather than guessed from what happens to be on screen.
    expect(engine).toMatch(/export type AwaitKind = false \| 'click' \| 'input'/)
    expect(overlay).toMatch(/awaiting === 'click'/)
    expect(hook).toMatch(/setAwaiting\('input'\)/)
    expect(hook).toMatch(/setAwaiting\('click'\)/)
  })

  it('sends the progress row back to the start between runs', () => {
    // The hook is not unmounted between runs - the shell keeps it mounted and
    // `replay` re-arms in place - so a chapter left where the last run stopped is
    // where the next one begins. Skipping at the worklog and replaying would open
    // on the first story beat with five dots already filled.
    const teardown = hook.slice(hook.indexOf('const finish = useCallback'))
    expect(teardown.slice(0, teardown.indexOf('setRunning(false)'))).toContain("setChapter('intro')")
  })

  it('still paints the way out on top of both fences', () => {
    // Carried from tutorial-click-fence: if a fence ever moves below Skip it
    // swallows it, and a user who cannot reach the target also cannot leave.
    // There are TWO now - the ringed cutout and the whole-viewport `FullFence`
    // added for narration beats - and Skip has to outrank both.
    const cutout = overlay.indexOf('<Cutout rect={ring} dim={spotlightDim} />')
    const full = overlay.indexOf('<FullFence />')
    const skip = overlay.indexOf('<button onClick={onSkip}')
    expect(cutout).toBeGreaterThan(-1)
    expect(full).toBeGreaterThan(-1)
    expect(skip).toBeGreaterThan(Math.max(cutout, full))
  })
})
