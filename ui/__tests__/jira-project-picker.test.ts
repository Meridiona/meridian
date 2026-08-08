//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'fs'

// Guards for the Jira project picker (feat/jira-project-picker): pulls every
// Jira project the connected account/site can see (via EITHER auth mode —
// browser OAuth or API token) and lets the user multi-select which ones sync,
// mirroring the GitHub Projects v2 picker (see github-connect-flow.test.ts).
//
// The repo has no React render harness (see oauth-setup-lifecycle.test.ts), so
// component behaviour is asserted by scanning the source for the required shape.

const uiRoot = import.meta.dir + '/..'
const view = readFileSync(uiRoot + '/components/IntegrationConnect.tsx', 'utf8')
const rustCmd = readFileSync(uiRoot + '/../tray/src-tauri/src/commands/integrations.rs', 'utf8')
const store = readFileSync(uiRoot + '/components/integrationConnectStore.ts', 'utf8')

// ── Backend: discovery works under either auth mode ──────────────────────────
describe('discover_jira_projects resolves either auth mode', () => {
  it('tries API-token basic auth first, then falls back to a stored OAuth session', () => {
    expect(rustCmd).toContain('async fn resolve_jira_ctx()')
    expect(rustCmd).toMatch(/has_basic[\s\S]*?JiraReqCtx::Basic/)
    expect(rustCmd).toContain('oauth_file_exists("jira")')
    expect(rustCmd).toContain('meridian_oauth::jira::ensure_fresh()')
  })

  it('paginates /rest/api/3/project/search until Jira reports isLast', () => {
    expect(rustCmd).toContain('"/rest/api/3/project/search"')
    expect(rustCmd).toMatch(/if is_last \|\| page_len == 0 \{\s*break;\s*\}/)
  })

  it('is registered as a tauri command the UI can invoke', () => {
    expect(rustCmd).toContain('pub async fn discover_jira_projects()')
  })
})

// ── Backend: a projects-only save must not require basic-auth fields ─────────
describe('save_integration_token accepts a projects-only save under OAuth', () => {
  it('bypasses the basic-auth required check when OAuth is connected and no basic field is submitted', () => {
    expect(rustCmd).toMatch(
      /if provider == "jira" && oauth_connected && !required\.iter\(\)\.any\(\|k\| updates\.contains_key\(\*k\)\)\s*\{\s*return Vec::new\(\)/
    )
  })

  it('still validates a real (re)connect attempt even when OAuth already exists', () => {
    // oauth_connected only excuses a PURE projects-only submit — a payload that
    // includes a basic-auth field must still go through the normal filter below.
    expect(rustCmd).toMatch(/oauth_connected[\s\S]{0,400}required\s*\.iter\(\)\s*\.copied\(\)\s*\.filter/)
  })

  it('keeps project_keys → JIRA_PROJECT_KEYS in TOKEN_FIELD_MAP', () => {
    expect(rustCmd).toContain('("project_keys", "JIRA_PROJECT_KEYS")')
  })

  it('strips JIRA_PROJECT_KEYS on disconnect alongside the other jira keys', () => {
    expect(rustCmd).toMatch(/"jira",\s*&\[\s*"JIRA_BASE_URL",\s*"JIRA_EMAIL",\s*"JIRA_API_TOKEN",\s*"JIRA_PROJECT_KEYS",/)
  })

  it('does not delete a live OAuth session on a projects-only submit', () => {
    // Regression guard: an earlier version of this gate deleted jira.json for
    // ANY jira save, which would destroy the very OAuth session a
    // projects-only submit rides on.
    expect(rustCmd).toMatch(/is_basic_auth_submit[\s\S]{0,200}if provider == "jira" && is_basic_auth_submit/)
  })
})

// ── Frontend: the picker appears at every entry point ─────────────────────────
describe('JiraProjectPicker is wired into every Jira connect path', () => {
  it('renders after a browser-OAuth connect completes', () => {
    expect(view).toMatch(/status === 'done'[\s\S]*?tracker\.id === 'jira'[\s\S]*?<JiraProjectPicker onSuccess=\{onSuccess\} \/>/)
  })

  it('renders after an API-token connect completes', () => {
    expect(view).toMatch(/tracker\.id === 'github' \|\| tracker\.id === 'jira'[\s\S]*?<JiraProjectPicker onSuccess=\{onSuccess\} \/>/)
  })

  it('renders from the "no projects selected" nudge in ConnectedPanel', () => {
    expect(view).toContain('No Jira projects selected - tasks won&apos;t sync yet.')
    expect(view).toMatch(/needsJiraProjects[\s\S]*?setPickingProjects\(true\)/)
  })

  it('defers onSuccess for jira exactly like github, in both OAuth and token flows', () => {
    expect(view).toMatch(/if \(status === 'done' && tracker\.id !== 'github' && tracker\.id !== 'jira'\) onSuccess\?\.\(\)/)
    expect(view).toMatch(/if \(tracker\.id !== 'github' && tracker\.id !== 'jira'\) onSuccess\?\.\(\)/)
  })
})

// ── Store: same retry-safety guard as the GitHub picker ──────────────────────
describe('jiraEnsureLoaded allows retrying after a failed discovery', () => {
  it('does not skip a re-fetch when the last attempt ended in loadError', () => {
    // Same bug class githubEnsureLoaded guards against: a failed discovery
    // must not latch permanently and block every later connect attempt.
    expect(store).toMatch(/export function jiraEnsureLoaded\(\): void \{\s*const s = jpGet\(\)\s*if \(\(s\.loaded && !s\.loadError\) \|\| s\.saving\) return/)
  })
})

// ── Store: the save submits project KEYS, not numeric ids ────────────────────
describe('jiraSave submits project keys', () => {
  it('maps the selected ids back to their keys before saving', () => {
    expect(store).toMatch(/const keys = \(projects \?\? \[\]\)\.filter\(\(p\) => selected\.has\(p\.id\)\)\.map\(\(p\) => p\.key\)/)
    expect(store).toContain('fields: { project_keys: keys.join(\',\') }')
  })
})

// ── Presentation: both pickers use the app's own control, not the OS's ───────
describe('project selection is a Meridian control', () => {
  const row = readFileSync(uiRoot + '/components/ui/SelectableRow.tsx', 'utf8')

  it('neither picker renders a bare native checkbox', () => {
    // A raw `<input type="checkbox">` renders at the OS's size in the OS's blue,
    // ignores every token in globals.css, and gives a ~13px hit target next to a
    // 12px label - on the screen where someone is first setting Meridian up, in a
    // list they have to hit several times.
    const pickers = view.slice(view.indexOf('function GitHubProjectPicker'))
    expect(pickers).not.toContain('type="checkbox"')
    expect(pickers).toContain('<SelectableRow')
  })

  it('the shared row keeps the native input for keyboard and a11y', () => {
    // Hidden, not replaced. A `<div onClick>` would drop Tab, Space and the
    // checked semantics, and then have to reimplement all three worse.
    expect(row).toContain('type="checkbox"')
    expect(row).toContain('className="peer sr-only"')
    expect(row).toContain('peer-focus-visible:outline')
  })

  it('the selected state is carried by the row, not only by a small glyph', () => {
    // Scannable down a list at a glance - the reason the whole surface tints.
    expect(row).toMatch(/background: checked \?/)
    expect(row).toMatch(/border: `1px solid \$\{checked \?/)
  })
})
