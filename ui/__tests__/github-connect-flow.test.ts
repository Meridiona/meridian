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
const store = readFileSync(uiRoot + '/components/integrationConnectStore.ts', 'utf8')

// ── PAT flow metadata (ui/lib/integrations.ts) ───────────────────────────────
describe('GitHub PAT metadata', () => {
  const gh = TRACKER_BY_ID.github
  const token = gh.token!

  it('pre-selects the required scopes and a token name in the create-PAT link', () => {
    expect(token.url).toContain('github.com/settings/tokens/new')
    expect(token.url).toContain('scopes=repo,read:org,project')
    expect(token.url).toContain('description=Meridian')
  })

  // Regression (issue #843): under the read-only `read:project` scope the
  // `addProjectV2ItemById` mutation that puts a newly created task on the user's
  // board is refused, so the issue never syncs back into Meridian. The PAT link
  // must ask for the read-write `project` scope, matching REQUIRED_SCOPES.
  it('asks for the read-write project scope, not read-only read:project', () => {
    expect(token.url).not.toContain('read:project')
    expect(token.hint).not.toContain('read:project')
  })

  it('drops the manual "Project IDs" node-ID field — only the token is asked for', () => {
    expect(token.fields.map((f) => f.name)).toEqual(['token'])
    expect(token.fields.some((f) => f.name === 'project_ids')).toBe(false)
  })

  it('tells the user the scopes are already selected so they just pick an org/repo', () => {
    expect(token.hint).toContain('repo, read:org, project')
    expect(token.hint.toLowerCase()).toContain('organisation or repositories')
  })

  it('keeps the browser (device) flow labelled "Browser" and code-based', () => {
    expect(gh.oauth!.label).toBe('Browser')
    expect(gh.oauth!.hint.toLowerCase()).toContain('one-time code')
  })
})

// ── Device-flow guided checklist (IntegrationConnect.tsx) ────────────────────
describe('device-flow guided checklist', () => {
  it('renders three DeviceStep items — the first two self-ticking, the third waiting', () => {
    expect(view).toMatch(/<DeviceStep n=\{1\} state=\{copiedOnce \? 'done' : 'active'\}/)
    expect(view).toMatch(/<DeviceStep n=\{2\} state=\{opened \? 'done' : copiedOnce \? 'active' : 'todo'\}/)
    // Step 3 happens on GitHub (cross-origin) — it can never self-tick, so it
    // stays a spinner ('waiting'), not a checkbox.
    expect(view).toMatch(/<DeviceStep n=\{3\} state="waiting"/)
  })

  it('records a PERSISTENT copy tick separate from the transient label flash', () => {
    // copiedOnce gates step 2 and must not revert; copied is the 1.5s label flash.
    expect(view).toMatch(/const \[copiedOnce, setCopiedOnce\] = useState\(false\)/)
    expect(view).toMatch(/setCopiedOnce\(true\); setCopied\(true\)/)
    expect(view).toMatch(/copied \? '✓ Copied' : 'Copy code'/)
    expect(view).toMatch(/setTimeout\(\(\) => setCopied\(false\)/)
  })

  it('gates the Open-GitHub button on a copy, and that button is the sole opener', () => {
    // The button is disabled until the code is copied, then opens the verify URI
    // via the Tauri opener (target="_blank" is dropped in the webview).
    expect(view).toMatch(/disabled=\{!copiedOnce\}/)
    expect(view).toMatch(/openExternal\(verifyUri \?\? 'https:\/\/github\.com\/login\/device'\); setOpened\(true\)/)
    expect(view).toContain('Open GitHub ↗')
  })

  it('still explains the per-org Grant/Request, with an accessible schematic image', () => {
    expect(view).toMatch(/<strong>Grant<\/strong> \(or <strong>Request<\/strong>\)/)
    // The illustration is an inline SVG with an aria-label (not a real screenshot).
    expect(view).toContain('<GitHubGrantHint />')
    expect(view).toMatch(/role="img"[\s\S]*?aria-label="On GitHub's authorize page, click Grant or Request/)
  })

  it('draws the callout arrow as a straight shaft plus a single triangular head', () => {
    // Regression guard: the old head was two freehand relative line segments
    // whose directions didn't agree with each other or the shaft, so it
    // rendered as a broken zigzag instead of a triangle. A <line> + <polygon>
    // can't independently drift out of alignment the same way — the polygon's
    // three points are the sole source of the head's shape.
    expect(view).toMatch(/<line x1="232" y1="94" x2="199" y2="83"/)
    expect(view).toMatch(/<polygon points="191,80 198,87 201,79"/)
    // The old curve + freehand-triangle path pair must be gone, not just added
    // alongside — otherwise both would render.
    expect(view).not.toMatch(/<path d="M244 92 Q 210 96 194 82"/)
    expect(view).not.toMatch(/l -2 -8 z/)
  })
})

// ── Backend: the browser is no longer auto-opened (integrations.rs + sys.rs) ──
describe('device flow no longer auto-opens the browser', () => {
  it('drops the open_url_detached call so the UI button is the sole opener', () => {
    // The connect checklist gates opening GitHub on a copy; auto-opening the tab
    // up front would contradict that. Removed from start_oauth_github_device.
    expect(rustCmd).not.toContain('open_url_detached(&device.verification_uri)')
  })

  it('removes the now-orphaned open_url_detached helper itself (no dead code)', () => {
    const sys = readFileSync(uiRoot + '/../tray/src-tauri/src/sys.rs', 'utf8')
    expect(sys).not.toContain('fn open_url_detached')
    expect(sys).not.toContain('fn is_web_url')
  })
})

// ── Project picker continues the checklist as Step 4 (IntegrationConnect.tsx) ─
describe('the project picker appears as the next step, not a separate screen', () => {
  it('collapses steps 1-3 to done ticks and renders the picker inside Step 4', () => {
    expect(view).toMatch(
      /status === 'done' && \([\s\S]*?<DeviceStep n=\{1\} state="done"[\s\S]*?<DeviceStep n=\{2\} state="done"[\s\S]*?<DeviceStep n=\{3\} state="done"[\s\S]*?<DeviceStep n=\{4\} state="active"[\s\S]*?<GitHubProjectPicker onSuccess=\{onSuccess\} \/>/
    )
  })

  it('uses a plain hyphen, not an em-dash, in the PAT-flow reload-warning text', () => {
    // CLAUDE.md hard rule: user-facing app text uses `-` only, never `—`.
    // This string is new code from this PR (copy-pasted from the sibling
    // reload-warning in the non-github done branch, which predates the PR and
    // is out of scope here) — caught in review, regression-guarded here.
    expect(view).toContain('The daemon wasn&apos;t running - credentials saved, will take effect on next start.')
  })
})

// ── PAT flow → project picker wiring (IntegrationConnect.tsx) ─────────────────
describe('PAT flow shows the project picker after save', () => {
  it('renders GitHubProjectPicker in TokenSetup for github once the token is saved', () => {
    // Jira shares this done-branch (its token/OAuth connect also needs a
    // project picked), so the github check is now one arm of an || rather
    // than a standalone condition — the GitHubProjectPicker render itself is
    // unchanged.
    expect(view).toMatch(/if \(tracker\.id === 'github' \|\| tracker\.id === 'jira'\) \{[\s\S]*?<GitHubProjectPicker onSuccess=\{onSuccess\} \/>/)
  })

  it('defers onSuccess for github so the picker is not unmounted before a board is picked', () => {
    expect(view).toMatch(/if \(tracker\.id !== 'github' && tracker\.id !== 'jira'\) onSuccess\?\.\(\)/)
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

// ── Regression: a failed discovery must not permanently block a retry ────────
describe('githubEnsureLoaded allows retrying after a failed discovery', () => {
  it('does not skip a re-fetch when the last attempt ended in loadError', () => {
    // THE BUG: the .catch() branch sets loaded:true on FAILURE too (so the UI
    // isn't stuck spinning forever), but the old guard `if (s.loaded || s.saving)
    // return` treated that identically to a SUCCESSFUL load. Once any single
    // discovery attempt failed, the store latched permanently: every later
    // connect attempt (a fresh PAT, a fresh OAuth run) silently reused the same
    // stale error, because resetGithubPickerIfSaved() only clears state once a
    // save succeeds — which can never happen while the picker shows no projects.
    expect(store).toMatch(/if \(\(s\.loaded && !s\.loadError\) \|\| s\.saving\) return/)
    // The old, buggy guard must be gone, not just superseded.
    expect(store).not.toMatch(/if \(s\.loaded \|\| s\.saving\) return/)
  })
})

// ── The picker offers a manual Retry, not just collapse/reopen ───────────────
describe('a failed discovery shows an explicit Retry action', () => {
  it('renders a Retry button that calls githubEnsureLoaded again', () => {
    expect(view).toMatch(/if \(loadError\) return \([\s\S]*?onClick=\{\(\) => githubEnsureLoaded\(\)\}[\s\S]*?Retry/)
  })
})

// ── PAT flow: the token-page link is a real button ────────────────────────────
describe('the token-creation link is a button, not an inline text link', () => {
  it('renders "Open <Tracker> ↗" as a styled button calling openExternal', () => {
    expect(view).toMatch(/<button\s+onClick=\{\(\) => openExternal\(method\.url!\)\}[\s\S]*?Open \{tracker\.name\} ↗/)
  })

  it('no longer buries the link inside the hint paragraph', () => {
    // The hint and the Open button are now two separate elements, not one
    // paragraph with an inline <a> — easier to see, and unambiguously clickable.
    expect(view).not.toMatch(/\{method\.hint\}\{' '\}\s*\{method\.url && \(\s*<a /)
  })
})
