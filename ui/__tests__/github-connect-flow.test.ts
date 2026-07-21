//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'fs'
import { TRACKER_BY_ID } from '../lib/integrations'

// Guards for the reworked GitHub connect flow (feat/github-connect-flow):
//   - Browser (device) flow: a Copy button that flips to "Copied", and the
//     code entry split into two clearly numbered steps.
//   - PAT flow: the token link pre-selects the scopes so the user only picks an
//     org/repo, the manual "Project IDs" node-ID field is gone, and after the
//     token is saved the same discover-and-tick GitHubProjectPicker the browser
//     flow uses is shown (no more pasting PVT_ node IDs).
//
// The repo has no React render harness (see oauth-setup-lifecycle.test.ts), so
// component behaviour is asserted by scanning the source for the required shape;
// the metadata (a plain TS object) is imported and asserted directly.

const uiRoot = import.meta.dir + '/..'
const view = readFileSync(uiRoot + '/components/IntegrationConnect.tsx', 'utf8')
const rustCmd = readFileSync(uiRoot + '/../tray/src-tauri/src/commands/integrations.rs', 'utf8')

// ── PAT flow metadata (ui/lib/integrations.ts) ───────────────────────────────
describe('GitHub PAT metadata', () => {
  const gh = TRACKER_BY_ID.github
  const token = gh.token!

  it('pre-selects the required scopes and a token name in the create-PAT link', () => {
    expect(token.url).toContain('github.com/settings/tokens/new')
    expect(token.url).toContain('scopes=repo,read:org,read:project')
    expect(token.url).toContain('description=Meridian')
  })

  it('drops the manual "Project IDs" node-ID field — only the token is asked for', () => {
    expect(token.fields.map((f) => f.name)).toEqual(['token'])
    expect(token.fields.some((f) => f.name === 'project_ids')).toBe(false)
  })

  it('tells the user the scopes are already selected so they just pick an org/repo', () => {
    expect(token.hint).toContain('repo, read:org, read:project')
    expect(token.hint.toLowerCase()).toContain('organisation or repositories')
  })

  it('keeps the browser (device) flow labelled "Browser" and code-based', () => {
    expect(gh.oauth!.label).toBe('Browser')
    expect(gh.oauth!.hint.toLowerCase()).toContain('one-time code')
  })
})

// ── Device-flow copy button + numbered steps (IntegrationConnect.tsx) ─────────
describe('device-flow code entry UI', () => {
  it('flips the copy button to a "Copied" state on click', () => {
    expect(view).toMatch(/setCopied\(true\)/)
    expect(view).toMatch(/copied \? '✓ Copied' : 'Copy'/)
    // The flourish is local, momentary state — reverted on a timer.
    expect(view).toMatch(/setTimeout\(\(\) => setCopied\(false\)/)
  })

  it('presents copy, enter, and per-org grant as three numbered steps', () => {
    expect(view).toContain('Step 1.')
    expect(view).toContain('Step 2.')
    expect(view).toContain('Step 3.')
  })

  it('warns that org access must be granted per-org on GitHub (it cannot be pre-granted)', () => {
    expect(view).toMatch(/GitHub requires this per organisation/)
  })

  it('opens the verification link through the Tauri opener, not a raw new tab', () => {
    // target="_blank" is dropped inside the Tauri webview (see external-links);
    // openExternal routes through the opener plugin instead.
    expect(view).toMatch(/openExternal\(verifyUri \?\? 'https:\/\/github\.com\/login\/device'\)/)
  })
})

// ── PAT flow → project picker wiring (IntegrationConnect.tsx) ─────────────────
describe('PAT flow shows the project picker after save', () => {
  it('renders GitHubProjectPicker in TokenSetup for github once the token is saved', () => {
    expect(view).toMatch(/if \(tracker\.id === 'github'\) \{[\s\S]*?<GitHubProjectPicker onSuccess=\{onSuccess\} \/>/)
  })

  it('defers onSuccess for github so the picker is not unmounted before a board is picked', () => {
    expect(view).toMatch(/if \(tracker\.id !== 'github'\) onSuccess\?\.\(\)/)
  })
})

// ── Backend contract the picker still depends on (integrations.rs) ────────────
describe('backend still accepts the project_ids the picker submits', () => {
  it('keeps project_ids → GITHUB_PROJECT_IDS in TOKEN_FIELD_MAP', () => {
    // The UI field was removed, but GitHubProjectPicker.githubSave() submits
    // { project_ids } to save_integration_token — so the Rust mapping MUST stay.
    expect(rustCmd).toContain('("project_ids", "GITHUB_PROJECT_IDS")')
  })
})
