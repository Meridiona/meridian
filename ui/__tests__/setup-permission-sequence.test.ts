//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The permissions step marks ONE card as "up next" and advances the ring as
// each grant lands, instead of presenting three equal cards with three equal
// Open Settings buttons.
//
// Source-scanning rather than behavioural, for the usual reason: the thing
// under test is which card carries an accent ring, and a headless run has no
// layout to read it off. What CAN be pinned is the shape of the logic that
// picks it - which is where every regression here would live.
//
// The load-bearing assertion is the last one. The obvious way to build a
// sequence is to disable the later cards, and that is a trap: macOS routinely
// does not report a TCC grant until the app restarts, so a lagging probe would
// leave the user staring at two dead buttons with no way to finish the step.
// The ring must be decoration over a fully live step, never a gate.

import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'fs'
import { join } from 'path'

const steps = readFileSync(join(import.meta.dir, '..', 'app', 'setup', 'steps.tsx'), 'utf8')

/** Body of `PermCard`, where the per-card styling decisions live. */
function permCard(): string {
  const at = steps.indexOf('function PermCard(')
  if (at < 0) throw new Error('marker not found: function PermCard(')
  const end = steps.indexOf('\nfunction ', at + 1)
  if (end < 0) throw new Error('end of PermCard not found')
  return steps.slice(at, end)
}

describe('the permissions step reads as a sequence', () => {
  it('picks the FIRST card that is neither granted nor hidden', () => {
    // `.find` (not `.filter`/`.some`) is what makes it the first one, and the
    // spread puts Notifications last - matching the left-to-right order the
    // three cards actually render in.
    expect(steps).toContain('const currentId = [...requiredCards, notifCard].find(')
    expect(steps).toMatch(/!permGranted\(p, wiz\) && !permHidden\(p, wiz\)/)
    // Nothing pending → no ring anywhere, rather than defaulting to card one
    // and re-ringing a step the user has already completed.
    expect(steps).toMatch(/\)\?\.id \?\? null/)
  })

  it('asks one shared predicate whether a permission is granted', () => {
    // PermissionsBody picks the current card and PermCard styles itself; if
    // each inlined its own granted-check they would drift, and the visible
    // result is a ring sitting on a green card while the genuinely pending one
    // looks finished.
    expect(steps).toContain('function permGranted(')
    expect(steps).toContain('function permHidden(')
    // The card must CALL it, not re-derive it.
    expect(permCard()).toContain('const granted = permGranted(p, wiz)')
    expect(permCard()).not.toMatch(/const granted = notif \?/)
  })

  it('rings the current card without resizing it', () => {
    const card = permCard()
    // A box-shadow ring, not a thicker border: a border that changes width
    // when the ring turns on would shunt the other two cards sideways every
    // time the sequence advances.
    expect(card).toMatch(/boxShadow: current \?/)
    expect(card).not.toMatch(/border: `?[0-9.]+px solid \$\{current/)
  })

  it('never disables a permission button', () => {
    // THE ONE THAT MATTERS. macOS grants frequently are not visible to the
    // probe until a relaunch, so gating the later cards on the earlier ones
    // would strand a user who has genuinely granted everything. Highlighting
    // is allowed to imply an order; it is not allowed to enforce one.
    const card = permCard()
    expect(card).not.toContain('disabled')
    // Both buttons switch variant on `current` - the sequence shows up in the
    // button too, and that is the whole of what `current` may do to them.
    const variants = card.match(/variant=\{current \? 'primary' : 'secondary'\}/g) ?? []
    expect(variants.length).toBe(2)
  })
})
