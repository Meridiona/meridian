//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// macOS Notifications is a DIFFERENT System Settings pane from Accessibility
// and Screen Recording, and the wizard's preview art has to say so.
//
// Accessibility and Screen Recording are one flat allow-list: a row per app
// with a toggle is a faithful picture of them. Notifications is two levels
// deep - the list only gets you to an app, and the switch the user actually
// has to flip lives on that app's own page under "Allow notifications". The
// three previews were all rendered from the same app-list component, so the
// Notifications card showed people a screen they would never land on, and
// then told them to look for a row that is not on it.
//
// Source-scanning: the failure here is visual (wrong picture, right code
// path), which no headless assertion can see. What it can pin is that the two
// pane shapes stay distinct - the regression is someone folding them back into
// one component because they look similar in the source.

import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'fs'
import { join } from 'path'

const atoms = readFileSync(join(import.meta.dir, '..', 'app', 'setup', 'atoms.tsx'), 'utf8')

describe('the permission preview art matches the pane it names', () => {
  it('routes macOS Notifications to its own pane component', () => {
    expect(atoms).toContain('function MacNotificationsPanePreview()')
    // The branch, not just the component's existence - an orphaned component
    // would satisfy the line above while every card still rendered the list.
    expect(atoms).toMatch(/permissionId === 'notifications'\s*\?\s*<MacNotificationsPanePreview \/>/)
  })

  it('shows the control the user actually has to flip', () => {
    // "Allow notifications" is the heading on the real per-app page. If this
    // string goes missing the picture has stopped being that page.
    expect(atoms).toContain('Allow notifications')
  })

  it('keeps the app-list pane for the two permissions that really are a list', () => {
    expect(atoms).toContain('<MacSettingsPanePreview permissionId={permissionId} />')
    // Accessibility and Screen Recording keep their blurred other-app rows;
    // that list is what makes THOSE two recognisable.
    expect(atoms).toMatch(/accessibility: \{[\s\S]*?others: \['Terminal', 'Google Chrome'\]/)
  })

  it('never presents the blurred scenery as something to click', () => {
    // Everything below the Allow-notifications row is landmark, not control.
    // Rendering it sharp would invite clicks on a picture.
    const at = atoms.indexOf('function MacNotificationsPanePreview()')
    const end = atoms.indexOf('\n/** The simple single-row preview', at)
    if (at < 0 || end < 0) throw new Error('MacNotificationsPanePreview bounds not found')
    const pane = atoms.slice(at, end)
    expect(pane).toContain("filter: 'blur(2px)'")
    expect(pane).not.toContain('onClick')
  })
})
