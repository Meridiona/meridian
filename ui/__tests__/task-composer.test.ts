//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'fs'

// Guards for the task composer's two load-bearing invariants.
//
// 1. THE AI CAN NEVER BLOCK A CREATE. Drafting is an LLM call that can take most of
//    a minute and can fail outright (a cold model, no provider configured, a copilot
//    backend that gives no schema guarantee at all). If the fields were gated behind
//    a successful draft, every one of those turns the morning planner back into the
//    dead end this feature exists to remove. So: the fields render unconditionally,
//    and `canCreate` looks at the title and nothing else.
//
// 2. COMPOSER STATE LIVES IN A MODULE STORE, not component state. The composer is
//    inside the Plan modal, which the user can close (Escape, backdrop, the X) while
//    a draft is in flight — component state would take the in-flight request and the
//    note they typed with it. Same hazard, and same fix, as useWorklog/planStore.
//
// No React render harness in this repo (see oauth-setup-lifecycle) — we model the
// store's predicate and scan the source for the required shape.

const uiRoot = import.meta.dir + '/..'
const composer = readFileSync(`${uiRoot}/components/plan/TaskComposer.tsx`, 'utf8')
const store = readFileSync(`${uiRoot}/components/plan/useTaskComposer.ts`, 'utf8')
const notice = readFileSync(`${uiRoot}/components/plan/AiEngineNotice.tsx`, 'utf8')

/** The real predicate, mirrored (the source is the contract; this pins its shape). */
const MIN_TITLE_WORDS = 4
const titleWords = (t: string) => t.trim().split(/\s+/).filter(Boolean).length
const canCreate = (s: { title: string; description: string; phase: string }) =>
  titleWords(s.title) >= MIN_TITLE_WORDS &&
  s.description.trim().length > 0 &&
  s.phase === 'idle'

/** A submittable task, for tests that vary one field off it. */
const ok = { title: 'Draft the Q3 roadmap deck', description: 'Put the deck together.', phase: 'idle' }

describe('the composer never blocks on the AI', () => {
  it('typed fields alone are enough to create - no draft required', () => {
    expect(canCreate(ok)).toBe(true)
  })

  it('a blank or whitespace title blocks', () => {
    expect(canCreate({ ...ok, title: '' })).toBe(false)
    expect(canCreate({ ...ok, title: '   ' })).toBe(false)
  })

  it('a title under the floor blocks - it names a subject, not the work', () => {
    // The floor the draft prompt also enforces, so AI and manual entry agree.
    expect(canCreate({ ...ok, title: 'Roadmap' })).toBe(false)
    expect(canCreate({ ...ok, title: 'Login bug' })).toBe(false)
    expect(canCreate({ ...ok, title: 'Fix broken login' })).toBe(false)
    // …and it counts WORDS, not characters: padding whitespace doesn't buy a pass.
    expect(canCreate({ ...ok, title: 'Roadmap                ' })).toBe(false)
    expect(canCreate({ ...ok, title: 'Fix the broken login' })).toBe(true)
  })

  it('a missing description blocks - a task you cannot decode next week is not one', () => {
    expect(canCreate({ ...ok, description: '' })).toBe(false)
    expect(canCreate({ ...ok, description: '  ' })).toBe(false)
  })

  it('an in-flight draft or create blocks a second submit', () => {
    expect(canCreate({ ...ok, phase: 'drafting' })).toBe(false)
    expect(canCreate({ ...ok, phase: 'creating' })).toBe(false)
  })

  it('canCreate never consults the draft or the note - only what is in the fields', () => {
    const src = store.slice(store.indexOf('export const canCreate'))
    const body = src.slice(0, src.indexOf('\n\n'))
    for (const forbidden of ['Drafted', 'note', 'error', 'created']) {
      expect(body.includes(forbidden)).toBe(false)
    }
  })

  it('a failed draft sets an error and leaves the fields alone', () => {
    // The draft path's failure arms must never touch title/description — that is
    // what makes a dead model a slow suggestion rather than a lost task.
    //
    // Checked by what the arms CONTAIN rather than by their exact text: they carry
    // an `errorSource` now (so the composer can tell a dead engine from a failed
    // create), and pinning the literal string made adding that look like a
    // regression when the invariant it guards was untouched.
    const fn = store.slice(store.indexOf('export function draftFromNote'), store.indexOf('export function createTask'))
    for (const arm of ['error: d.error', 'error: errMsg(e)']) {
      const at = fn.indexOf(arm)
      expect(at).toBeGreaterThan(-1)
      // The whole `patch({...})` the arm sits in, and nothing of the field-setting
      // success path above it.
      const call = fn.slice(fn.lastIndexOf('patch({', at), fn.indexOf('}', at) + 1)
      expect(call).toContain("phase: 'idle'")
      for (const field of ['title', 'description', 'issueType']) {
        expect(call.includes(field)).toBe(false)
      }
    }
  })

  // A draft can fail for two completely different reasons, and until now the
  // composer treated them the same: a red line of text with no way forward. The
  // pre-flight only ran on the hero (first-run) surface, so on the board column a
  // missing engine produced "Couldn't draft that - write it below." and nothing
  // else. These pin the two halves of the fix.
  it('probes for an AI engine on every mount, not just the first-run one', () => {
    // `if (hero)` here is the bug: the board column's composer is the same form for
    // the same person, and it is where a signed-out engine actually gets hit.
    expect(composer).toContain('useEffect(() => { void probeProviders() }, [probeProviders])')
    expect(/if\s*\(hero\)\s*void probeProviders/.test(composer)).toBe(false)
  })

  it('a draft that fails on a dead engine offers the connect route, not just text', () => {
    // The pre-flight cannot catch an engine that is configured but signed out - that
    // only surfaces when the call is made. So a draft failure re-asks health, and a
    // bad answer raises the same notice the pre-flight does.
    expect(composer).toContain("s.errorSource !== 'draft'")
    expect(composer).toContain('setShowAiNotice(true)')
    // And the raw failure is not printed underneath the notice that supersedes it.
    expect(composer).toContain('s.error && !showAiNotice')
  })

  it('a live engine failure beats what health remembers', () => {
    // The dead end this closes: health scores the LAST RECORDED TEST, so a provider
    // whose key tested fine reads `llm_provider_ok: true` while every real call fails.
    // The failure branch asked health, health said fine, no notice was raised, and the
    // user got "Try again" on a button that could only fail identically - with no route
    // to the picker from anywhere on the screen.
    //
    // So the draft call reports whether the ENGINE failed, and that answer wins outright
    // rather than being second-guessed by a remembered success.
    expect(store).toContain('providerDown: !!d.provider_down')
    expect(composer).toContain('if (s.providerDown) { setShowAiNotice(true)')
    // Ahead of the health probe, not after it - the whole point is not to ask.
    expect(composer.indexOf('if (s.providerDown)')).toBeLessThan(composer.indexOf('void probeProviders().then(ok =>'))
  })

  it('the engine says WHY, and the user sees it', () => {
    // Every model failure used to collapse to one fixed sentence, so an expired key, a
    // wrong model name and an exhausted quota were indistinguishable - three different
    // fixes behind identical words. The reason now rides the notice's headline.
    expect(composer).toContain('reason={s.providerDown ? s.error ?? undefined : undefined}')
    expect(notice).toContain("{reason ?? 'Meridian needs an AI engine'}")
    // …and the button stops telling someone who HAS a provider to connect one.
    expect(notice).toContain("reason ? 'Check your provider' : 'Connect a provider'")
  })

  it('only a DRAFT failure is treated as an AI problem', () => {
    // A create that fails on tracker permissions has nothing to do with the model;
    // offering to connect a provider there sends the user to the wrong screen.
    expect(store).toContain("errorSource: 'create'")
    expect(store).toContain("errorSource: 'draft'")
  })

  it('the title/description fields are not rendered behind a draft conditional', () => {
    // The fields must sit outside any `draft &&` / `hasDraft ?` gate. If this ever
    // trips, someone has re-gated the manual path behind the model.
    expect(/\bdraft(ed)?\s*&&\s*\(?\s*<input/i.test(composer)).toBe(false)
    expect(composer.includes('value={s.title}')).toBe(true)
    expect(composer.includes('value={s.description}')).toBe(true)
  })
})

// This file's own header already says drafting "can take most of a minute" - but the
// component used to override <GeneratingBar>'s honest default note ("this might take a
// minute or so", the same wording WorklogDraftDialog and SummaryTaskView show) with a
// hardcoded "this usually takes a few seconds". Cursor in particular can legitimately take
// closer to a minute on a cold or hardened-ladder-retried call, so that override was a
// promise the button could not keep for every provider - confirmed live (a single hardened
// cursor-agent call measured ~6s; a call that has to retry can run well past "a few
// seconds"). Fixed by dropping the override entirely.
describe('the drafting spinner does not promise a latency it cannot keep', () => {
  it('does not override GeneratingBar\'s note with an inaccurate "few seconds" claim', () => {
    const generatingBarBlock = composer.slice(composer.indexOf('<GeneratingBar'))
    expect(generatingBarBlock).not.toMatch(/note=/)
  })
})

describe('composer state survives the plan modal closing mid-draft', () => {
  it('the store is module-level and exposed via useSyncExternalStore', () => {
    expect(store.includes('useSyncExternalStore')).toBe(true)
    expect(store.includes('let state: ComposerState = EMPTY')).toBe(true)
  })

  it('the server snapshot is a stable frozen EMPTY (a fresh object loops forever)', () => {
    expect(store.includes('Object.freeze(')).toBe(true)
    expect(/\(\)\s*=>\s*EMPTY,/.test(store)).toBe(true)
  })

  it('the component holds no useState for composer fields', () => {
    // The point of this guard is that the fields the user has TYPED INTO survive
    // the component unmounting mid-flow — so it names them, rather than banning
    // useState outright. Ephemeral UI state that SHOULD reset on reopen (the
    // "is any AI provider installed" probe) is not what this protects, and
    // blanket-banning the hook pushed such state into the shared store, where a
    // stale value would then outlive the composer it belonged to.
    for (const field of ['title', 'description', 'note', 'issueType', 'target']) {
      const re = new RegExp(`useState[^\\n]*\\b${field}\\b|\\[\\s*${field}\\s*,\\s*set`, 'i')
      expect(re.test(composer)).toBe(false)
    }
  })

  it('personal is the default target - filing on a shared board is a deliberate click', () => {
    expect(store.includes('target: LOCAL_PROVIDER')).toBe(true)
  })
})

describe('nothing calls the plan-task commands outside the store', () => {
  it('draft/create/edit are invoked only from useTaskComposer', () => {
    const files = ['components/plan/TaskComposer.tsx', 'components/plan/PlanView.tsx',
      'components/plan/PlanBoardColumn.tsx', 'components/plan/PlanTodayColumn.tsx']
    for (const f of files) {
      const src = readFileSync(`${uiRoot}/${f}`, 'utf8')
      for (const cmd of ['draft_plan_task', 'create_plan_task', 'edit_plan_task']) {
        expect(src.includes(cmd)).toBe(false)
      }
    }
    expect(store.includes('draft_plan_task')).toBe(true)
    expect(store.includes('create_plan_task')).toBe(true)
  })
})

// A new user pressing "Draft with AI" with no provider connected used to get a
// red error string, then (briefly) an instant jump into Settings. Both are the
// same failure: the app changing state without saying why. These pin the shape
// of the replacement, all of which is invisible to a type checker.
describe('drafting with no AI provider connected', () => {
  const notice = readFileSync(`${uiRoot}/components/plan/AiEngineNotice.tsx`, 'utf8')
  const modalShell = readFileSync(`${uiRoot}/components/timeline/ModalShell.tsx`, 'utf8')

  it('explains before navigating - the jump is never the button\'s own doing', () => {
    // draftOrConnect may only OPEN THE NOTICE. If it dispatches the navigation
    // itself, the user is back to being teleported by a click on "Draft".
    const fn = composer.slice(composer.indexOf('const draftOrConnect'))
    const body = fn.slice(0, fn.indexOf('\n  const '))
    expect(body).toContain('setShowAiNotice(true)')
    expect(body).not.toContain('meridian:connect-ai')
    expect(notice.length).toBeGreaterThan(0)
  })

  it('is one line and one action - no second way to do nothing', () => {
    // The card has exactly one button. A dismiss would be a second control that
    // does what ignoring the card already does: the fields below stay editable
    // the whole time and "Add to today" never depended on a model, so there is
    // nothing to dismiss. Two buttons here spent the card's one clear call to
    // action arguing with itself.
    const buttons = notice.match(/<button/g) ?? []
    expect(buttons.length).toBe(1)
    expect(notice.replace(/\s+/g, ' ')).toMatch(/Meridian needs an AI engine/i)
    expect(notice).toContain('onConnect')
  })

  it('leaves the manual path untouched while it is showing', () => {
    // The notice renders inline, above the divider - never in place of the
    // title/description fields, which is what makes having no dismiss safe.
    const at = (s: string) => composer.indexOf(s)
    expect(at('showAiNotice &&')).toBeLessThan(at('value={s.title}'))
    expect(at('value={s.title}')).toBeGreaterThan(0)
  })

  it('a locked modal drops every INCIDENTAL exit but keeps a labelled one', () => {
    // Escape and the backdrop go; the corner button stays and is renamed. A lock
    // that removed the last way out would be a trap, which is never the intent.
    expect(modalShell).toContain('if (lock) return')          // no Escape handler
    expect(modalShell).toContain('onClick={lock ? undefined : onClose}')  // no backdrop
    expect(modalShell).toContain('{lock ? lock.label :')      // labelled, not an ×
    expect(modalShell).toContain('data-tour="modal-close"')   // still ends the tour beat
  })
})
