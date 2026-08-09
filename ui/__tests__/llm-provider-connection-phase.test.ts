//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The three states a provider card can be in, and what the user sees for each:
//
//   1. NOT INSTALLED AT ALL       — probed.installed === false
//   2. INSTALLED, NOT AUTHENTICATED — probed.installed === true, but the real "Test" call
//      (test_llm_provider -> src/llm/detect.rs::test_provider, a genuine live request to the
//      CLI — this IS the "send a live request and check the response" button) came back
//      failed. Meridian never claims to know auth state up front (`authenticated` is always
//      null — see api-types.ts) — "signed in or not" is only ever answered by actually trying
//      a real call, which is exactly what the Test button does.
//   3. INSTALLED, AUTHENTICATED, WORKING — the same real call came back `{status: 'ok'}`.
//
// `phaseFor` is imported and exercised for real — it is the single place these three states
// (plus the transient installing/signing/testing ones) collapse into what the card renders.
// The rest is scanned from source — this repo has no React render harness (see
// worklog-propose-provider.test.ts).

import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { BLOCKS_USE, phaseFor, statusPill } from '../components/LlmProviderDetail'
import {
  LLM_INTRO_BODY, LLM_RANK_FREE, LLM_RANK_SUBSCRIPTION, LLM_RECOMMENDED_NOTE, USAGE_FOOTPRINT_NOTE,
} from '../lib/llm-providers'
import type { ProviderStatus, ProviderTestResult } from '../lib/api-types'

const src = (p: string) => readFileSync(join(import.meta.dir, '..', p), 'utf8')
const detail = src('components/LlmProviderDetail.tsx')

const status = (installed: boolean, path: string | null = null): ProviderStatus => ({
  id: 'codex',
  installed,
  path,
  authenticated: null,
  last_test: null,
  install_command: 'npm i -g @openai/codex',
})

const testResult = (outcome: ProviderTestResult['outcome']): ProviderTestResult => ({
  id: 'codex',
  outcome,
  elapsed_ms: 1234,
  tested_at: '2026-07-24T12:00:00Z',
})

describe('phaseFor — scenario 1: not installed at all', () => {
  it('is not_installed once probed and absent, with no prior test on record', () => {
    expect(phaseFor(false, false, false, status(false), null).kind).toBe('not_installed')
  })

  it('is STILL not_installed even if a stale last_test says ok — a since-uninstalled CLI must not read as working', () => {
    const stale = testResult({ status: 'ok' })
    expect(phaseFor(false, false, false, status(false), stale).kind).toBe('not_installed')
  })

  it('is unknown, not not_installed, when the probe itself has not resolved yet (no false "missing" flash on mount)', () => {
    expect(phaseFor(false, false, false, undefined, null).kind).toBe('unknown')
  })
})

describe('phaseFor — scenario 2: installed, not authenticated', () => {
  it('is failed once a real Test call comes back failed', () => {
    const failed = testResult({ status: 'failed', message: 'not authenticated' })
    const phase = phaseFor(false, false, false, status(true, '/usr/local/bin/codex'), failed)
    expect(phase.kind).toBe('failed')
    expect(phase.kind === 'failed' && phase.message).toBe('not authenticated')
  })

  it('is ready_untested when installed but no Test has been run yet — the state before the user has clicked anything', () => {
    const phase = phaseFor(false, false, false, status(true, '/usr/local/bin/codex'), null)
    expect(phase.kind).toBe('ready_untested')
  })

  it('is rate_limited, not failed, when the account is signed in but temporarily capped — a different fix than "sign in"', () => {
    const limited = testResult({ status: 'rate_limited', message: 'usage limit' })
    const phase = phaseFor(false, false, false, status(true), limited)
    expect(phase.kind).toBe('rate_limited')
  })
})

describe('phaseFor — scenario 3: installed, authenticated, working', () => {
  it('is ok once a real Test call succeeds', () => {
    const ok = testResult({ status: 'ok' })
    expect(phaseFor(false, false, false, status(true, '/usr/local/bin/codex'), ok).kind).toBe('ok')
  })
})

describe('phaseFor — transient / in-flight states win over any stale result', () => {
  it('installing beats everything else', () => {
    const ok = testResult({ status: 'ok' })
    expect(phaseFor(true, false, false, status(true), ok).kind).toBe('installing')
  })

  it('signing beats everything else', () => {
    expect(phaseFor(false, true, false, status(true), null).kind).toBe('signing')
  })

  it('testing (the Test button, in flight) beats a stale prior result', () => {
    const failed = testResult({ status: 'failed', message: 'old failure' })
    expect(phaseFor(false, false, true, status(true), failed).kind).toBe('testing')
  })
})

// ── The Test button really does send one real request — not a canned/local check ──────────

it('onTest is wired straight to the real backend test command, not a local heuristic', () => {
  // detail.tsx takes onTest as a prop and never computes install/auth state itself — the
  // picker (one level up) owns the actual invoke() call. Assert the prop is threaded through
  // to both places a user can trigger it, so a future refactor can't quietly stub it out.
  expect(detail).toMatch(/onTest\s*:\s*\(\)\s*=>\s*void/)
  expect(detail).toMatch(/onClick=\{onTest\}/)
})

// The picker's surface spans two files: the component, and the detection hook that was split
// out of it to keep both under the 500-line rule. These assertions are about the behaviour,
// not about which file it lives in, so they scan both.
const picker = src('components/LlmProviderPicker.tsx') + src('components/useLlmProviderDetection.ts')

it("the picker's test handler invokes the real Tauri command against the live CLI, not a mock", () => {
  expect(picker).toMatch(/invoke<ProviderTestResult>\('test_llm_provider'/)
})

// ── Subscription providers (Cursor, Codex, Claude) get bespoke "not signed in" copy with an ──
// ── in-app sign-in button for an unclassified failure; every other provider shows the raw ────
// ── message. Both must still originate from the SAME real Test call (a `failed` phase). ──────

it('the subscription-provider "not signed in" copy only replaces the DISPLAY, not the trigger — it is still gated on phase.kind === "failed"', () => {
  const failedBranch = detail.slice(detail.indexOf("case 'failed':"))
  // The in-app sign-in UI is gated on the failed phase via the signInProvider descriptor,
  // showing the "not signed in" copy and that provider's own sign-in button label.
  expect(failedBranch).toMatch(/if \(signInProvider\)/)
  expect(failedBranch).toMatch(/not signed in yet/)
  expect(failedBranch).toMatch(/\{signInProvider\.label\}/)
  // Cursor, Codex, and Claude are each covered by the descriptor (each with its own label).
  // The labels live in the shared registry, not inline here - see the desync test below.
  const registry = src('lib/llm-providers.ts')
  expect(registry).toMatch(/Sign in to Cursor/)
  expect(registry).toMatch(/Sign in to Codex/)
  expect(registry).toMatch(/Sign in to Claude/)
})

// The button copy and the tray command it fires used to live in two files - a provider added
// to one and not the other got a sign-in button wired to nothing, or (worse) to another
// vendor's login. One record now carries both, so they cannot desync.
it('every in-app sign-in provider carries BOTH its copy and its tray command in one record', () => {
  const registry = src('lib/llm-providers.ts')
  for (const [id, cmd] of [['cursor', 'cursor_sign_in'], ['codex', 'codex_sign_in'], ['claude', 'claude_sign_in']]) {
    const entry = registry.slice(registry.indexOf(`  ${id}: {`))
    const block = entry.slice(0, entry.indexOf('},'))
    expect(block).toMatch(/label:/)
    expect(block).toMatch(new RegExp(`trayCommand: '${cmd}'`))
  }
  // ...and the picker must not have grown a second, competing map.
  expect(picker).not.toMatch(/SIGN_IN_COMMANDS/)
  expect(picker).toMatch(/llmSignIn\(id\)\?\.trayCommand/)
})

// ── The displayed install command must come from the PROBE, not the static meta hint ────────
//
// `LlmProviderMeta.installHint` is one string compiled into a dashboard that ships to macOS
// AND Windows, so it cannot be correct on both: it names the npm command, while Windows
// installs natively (see meridian-core/src/llm_provider.rs). It is also the "run it yourself"
// fallback shown AFTER a failed install — so sourcing it statically told a Windows user to run
// the exact command that cannot work there. `ProviderStatus.install_command` is the command
// the running backend will actually execute, so it is right on every platform by construction.

it('the install hint shown to the user comes from the probe, falling back to the static meta hint only until it resolves', () => {
  expect(detail).toMatch(/installHint=\{probed\?\.install_command \?\? p\.installHint\}/)
})

it('ProviderStatus carries the install command, so the UI never has to guess the platform', () => {
  const apiTypes = src('lib/api-types.ts')
  const block = apiTypes.slice(apiTypes.indexOf('export interface ProviderStatus'))
  expect(block.slice(0, block.indexOf('\n}'))).toMatch(/install_command: string \| null/)
})

it('the detail view does not fall back to the static hint anywhere else — one source, no second path', () => {
  // Every other `p.installHint` read would reintroduce the platform-wrong string. The single
  // permitted use is the `??` fallback asserted above.
  const reads = detail.match(/p\.installHint/g) ?? []
  expect(reads.length).toBe(1)
})

it('every other provider surfaces the real failure message verbatim, not a generic substitute', () => {
  // The heading names the provider; the body IS the message the backend returned. A generic
  // substitute ("something went wrong") would throw away the only text that says what to fix.
  const generic = detail.slice(detail.indexOf("isn&apos;t responding"))
  expect(generic.slice(0, 400)).toMatch(/\{phase\.message\}/)
})

// ── What this screen is allowed to CLAIM ─────────────────────────────────────────────────

describe('"is writing your summaries" is gated on proof, not on a stored string', () => {
  it('requires a passing test as well as being selected', () => {
    // `selected` means only "this id is in settings.json" - true of the DEFAULT provider
    // before anyone has touched anything. Reading it as "working" put a green "Claude Code
    // is writing your summaries" banner directly under a panel saying the CLI was not
    // signed in, on a first run where neither was true.
    expect(detail).toContain("const active = selected && phase.kind === 'ok'")
    // Every claim of working-ness hangs off `active`, never off `selected`.
    // lastIndexOf: the phrase is quoted in the file header too, explaining exactly this.
    const at = detail.lastIndexOf('is writing your summaries')
    expect(detail.slice(at - 900, at)).toContain('active ? (')
  })

  it('says where the provider actually stands, in one pill', () => {
    expect(statusPill({ kind: 'ok' }, true).label).toBe('IN USE')
    expect(statusPill({ kind: 'ok' }, false).label).toBe('CONNECTED')
    // The states that need the user to do something read as such - not as a neutral label.
    expect(statusPill({ kind: 'failed', message: 'x' }, false).label).toBe('ACTION NEEDED')
    expect(statusPill({ kind: 'not_installed' }, false).label).toBe('NOT INSTALLED')
  })

  it('refuses to commit a provider that has just been PROVED not to work', () => {
    // Writing a known-broken provider has exactly one visible effect: the hourly summaries
    // silently never run. In the walkthrough it would also release the required lock on a
    // step that is not done.
    for (const k of ['not_installed', 'failed', 'installing', 'signing', 'testing']) {
      expect(BLOCKS_USE).toContain(k as never)
    }
    // ...but an un-probed CLI is NOT blocked: it may well be installed and signed in, and
    // refusing it would strand anyone whose probe failed for an unrelated reason.
    expect(BLOCKS_USE).not.toContain('unknown' as never)
    expect(BLOCKS_USE).not.toContain('ready_untested' as never)
    expect(BLOCKS_USE).not.toContain('ok' as never)
    // Disabled with a REASON. A dead button with no explanation is the same dead end.
    expect(detail).toContain('disabled={applying || blocked}')
    expect(detail).toContain('Finish the step above and this turns on.')
  })
})

describe('the usage claim is made once, and says the same thing everywhere', () => {
  it('quotes one number, not one per screen', () => {
    // The grid said "less than 2% of your daily usage" and the detail screen one click later
    // said "well under 1%". Two different numbers for the same fact reads as marketing.
    const pct = (s: string) => s.match(/(\d+)%/)?.[1]
    expect(pct(USAGE_FOOTPRINT_NOTE)).toBe(pct(LLM_RECOMMENDED_NOTE)!)
  })

  it('is not ALSO made in the section intro, 40px above itself', () => {
    // The intro carried a third wording of it, in the same viewport as the note under the
    // grid and disagreeing with it. Reassurance repeated three times at three sizes stops
    // being reassurance.
    expect(LLM_INTRO_BODY).not.toMatch(/\d+%/)
  })
})

describe('the chooser grid', () => {
  const picker = src('components/LlmProviderPicker.tsx')

  it('is always a complete rectangle - 3 across gated, 2 x 2 in Settings', () => {
    // auto-fill laid four tiles out 3 + 1, which reads as "and one afterthought" and left the
    // last card a different shape from its siblings.
    expect(picker).toContain('repeat(${gate ? 3 : 2}, minmax(0, 1fr))')
  })

  it('carries the SAME ranking the gate showed, on every tile', () => {
    // The gate ranks the two paths, then the grid shows one path's tiles beside the other's.
    // Telling someone BEST ACCURACY on the question and then showing nothing on the tiles
    // hands them a ranking and takes it away at the moment they act on it. One record backs
    // both, and one <Badge> renders both.
    expect(LLM_RANK_SUBSCRIPTION.badge).toBe('RECOMMENDED')
    expect(LLM_RANK_SUBSCRIPTION.note).toBe('BEST ACCURACY')
    expect(LLM_RANK_FREE.badge).toBe('FREE')
    expect(LLM_RANK_FREE.note).toBe('GOOD ACCURACY')
    expect(picker).toContain('rank={LLM_RANK_SUBSCRIPTION}')
    expect(picker).toContain('rank={LLM_RANK_FREE}')
    expect(picker).toContain("import LlmProviderGate, { Badge } from '@/components/LlmProviderGate'")
    // The gate reads from the same record rather than repeating the strings.
    const registry = src('lib/llm-providers.ts')
    expect(registry).toContain('...LLM_RANK_SUBSCRIPTION')
    expect(registry).toContain('...LLM_RANK_FREE')
  })

  it('ranks by contrast - never by colour, never by a heavy dark chip', () => {
    // Every hue in this app already means something (green connected, violet AI, amber
    // attention) and a rank badge means none of them - the gate learnt that with a green
    // card next to a violet one. The next attempt, a near-black filled chip, avoided the
    // colour problem by becoming the loudest thing on the screen instead: at this size a
    // solid dark block is the heaviest mark available, and there are two of them per tile.
    const gate = src('components/LlmProviderGate.tsx')
    const badge = gate.slice(gate.indexOf('export function Badge'))
    expect(badge).not.toMatch(/--color-state-(approved|proposal|pending)/)
    expect(badge).not.toContain("background: filled ? 'var(--t-title)'")
    // A soft tint carrying full-strength text, against a bare outline carrying muted text.
    expect(badge).toContain("color: filled ? 'var(--t-title)' : 'var(--t-faint)'")
    expect(badge).toContain("background: filled ? 'var(--t-box)' : 'transparent'")
  })

  it('names the fourth tile Groq and says FREE, not "bring your own API key / ADVANCED"', () => {
    // That label described the plumbing, not the offer. Someone with no subscription - the
    // exact person the tile is for - could not tell from it that this is the free path, and
    // "advanced" actively warns them off. Groq is the only preset left, so the tile is Groq.
    expect(picker).toContain('name={GROQ.name}')
    // ...and it opens the SAME three-step walkthrough the gate's free answer lands on, not a
    // second, differently-shaped route to the same place.
    expect(picker).toContain('custom.providers.length === 0')
  })

  it('shows a rescan long enough to be believed', () => {
    // The probe answers in single-digit milliseconds, so the old bare underlined link
    // changed its label and changed back inside one frame: pressed, nothing moved, and the
    // only reasonable conclusion was that it does nothing. The floor is on the STATE, not
    // the work - results still land as soon as they arrive.
    const hook = src('components/useLlmProviderDetection.ts')
    expect(hook).toContain('PROBE_MIN_VISIBLE_MS')
    expect(hook).toContain('await floor')
    expect(picker).toContain("animation: 'spin 0.7s linear infinite'")
  })
})

describe('an already-working provider still releases the connect flow', () => {
  const intel = src('components/timeline/settings/IntelligenceSection.tsx')
  const modal = src('components/timeline/SettingsModal.tsx')

  it('satisfies the lock on PROOF, not only on a fresh write', () => {
    // `commit` used to be the only caller of `onConnected`, which assumed the user always
    // arrives with nothing working and leaves having written a choice. When the provider in
    // settings.json is already the right one and starts answering while they are on its
    // screen - the auto-test passing, or a silent re-test after sign-in - `active` flips and
    // LlmProviderDetail swaps the "Use <provider>" button for "<provider> is writing your
    // summaries". No control is left to press, so the callback could never fire and the
    // walkthrough waited forever on a screen that said everything was fine.
    expect(intel).toContain("if (phase.kind !== 'ok') return")
    expect(intel).toContain('handedBack.current = provider')
    expect(intel).toContain('onConnected()')
  })

  it('decides "connected" with the same function the button does', () => {
    // Restating the condition here would let this and the detail view's `active` drift, and
    // the failure would be silent in the worst direction: a screen showing IN USE while the
    // flow refuses to move on.
    expect(intel).toContain("import { phaseFor } from '@/components/LlmProviderDetail'")
  })

  it('only hands back when the user was SENT here to connect', () => {
    // `onConnected` releases the modal and drops the user into the planner. Under the gate
    // that is the whole point; opening Settings from the toolbar to look at a provider that
    // has worked for weeks must not do it.
    expect(intel).toContain('if (!gate || !onConnected) return')
  })

  it('says the step is done before the screen moves', () => {
    // Otherwise the modal just vanishes and the planner reappears - the user is never told
    // the thing they were interrupted for is finished, or that the app is putting them back
    // on purpose.
    expect(modal).toContain('function ConnectedHandBack')
    expect(modal).toContain("Great - that's your AI connected. Taking you back to your task…")
    expect(modal).toMatch(/\{outcome && lock && <ConnectedHandBack outcome=\{outcome\} section=\{initialSection\}/)
  })

  it('holds that line long enough to read', () => {
    expect(modal).toContain('const HAND_BACK_MS = 2200')
    expect(modal).toContain('HAND_BACK_MS)')
  })

  it('gets out of the way faster when the answer was "I do not use one"', () => {
    // That path owes the user a real explanation - you can still do all of this,
    // with AI, right here - and it is given properly by the walkthrough on a
    // full-screen card once the planner is back. Holding the modal for the full
    // 2.2s first makes them read a cramped version and then read it again.
    expect(modal).toContain('const DECLINE_HAND_BACK_MS = 1100')
    // Asserted as the BRANCH rather than as one ternary: the connected path now also
    // waits on the first ticket sync, so the two holds are no longer a single
    // expression. What must stay true is that declining takes the short one.
    expect(modal).toMatch(/if \(outcome === 'declined'\) \{[\s\S]{0,160}setTimeout\(onLockSatisfied, DECLINE_HAND_BACK_MS\)/)
  })

  it('the connected path absorbs the first ticket sync instead of the planner doing it', () => {
    // The wait is the same length either way; where it is spent is the whole point.
    // Paid on the planner, the user reaches the screen that exists to show their
    // tickets and finds it empty. Paid here, the screen already says something true
    // and finished, and the planner opens onto a full board.
    expect(modal).toContain('pendingTaskSync()')
    expect(modal).toContain("section === 'integrations' ? pendingTaskSync() : null")
    // Capped - this modal is LOCKED while it waits, so a stalled tracker must never
    // be able to strand someone in it.
    expect(modal).toMatch(/const SYNC_HOLD_MAX_MS = \d+/)
    expect(modal).toContain('Promise.race([pending, after(SYNC_HOLD_MAX_MS)])')
  })

  it('uses a plain hyphen in the confirmation, like every other user-facing string', () => {
    const line = modal.slice(modal.indexOf('Great - '), modal.indexOf('Taking you back') + 40)
    expect(line).not.toMatch(/[—–]|--/)
  })
})

describe('the resumed composer keeps the note and drafts it', () => {
  const composer = src('components/plan/TaskComposer.tsx')
  const store = src('components/plan/useTaskComposer.ts')

  it('asks whether it is resuming once per MOUNT, not once per effect run', () => {
    // `consumeResume` is one-shot, and `reactStrictMode` (next.config.ts) makes React run
    // this effect, tear it down, and run it again on the same instance. The second run found
    // the flag spent, took it for an ordinary open, and called `resetComposer()` - so the
    // branch that exists to PRESERVE the note across the Settings detour was the thing that
    // wiped it, and the draft never started. Connect a provider, land on an empty box.
    expect(composer).toContain('const resuming = useRef<boolean | null>(null)')
    expect(composer).toContain('if (resuming.current === null) resuming.current = consumeResume()')
    expect(composer).toContain('if (resuming.current) {')
  })

  it('still resets on an ordinary open', () => {
    // The guard must not turn into "never reset" - a half-typed task from an hour ago is
    // still not what the user wants back when they open the composer fresh.
    expect(composer).toContain('resetComposer()')
  })

  it('runs the draft the user already asked for, off the note it kept', () => {
    expect(composer).toContain('if (s.note.trim()) draftFromNote()')
  })

  it('keeps the flag one-shot at the store, where that IS the contract', () => {
    // The fix belongs in the consumer, not here: a flag that survived being read would
    // re-trigger a draft on the next unrelated open.
    expect(store).toContain('resumePending = false')
  })
})

// ── ONE ladder, one implementation ───────────────────────────────────────────────────────
//
// "Is the AI provider connected" had four separate answers in this codebase, and they
// disagreed on screen:
//
//   1. Rust `classify_provider_health` → `get_health.llm_provider_ok` — the banner, the task
//      composer, the worklog dialog, the day-summary view, and the resolver's own gate.
//   2. `phaseFor` here — the provider detail screen and the Settings lock.
//   3. An inline ternary in <ChooserTile>'s call site — the grid. Had no notion of
//      `rate_limited`, so a throttled provider showed a confident green tile while the banner
//      said it was catching up.
//   4. Nothing at all for the cloud endpoint: the Groq tile and the endpoint card both read
//      `value === 'custom'`, i.e. a fact about settings.json, and rendered a green IN USE for
//      a provider the app had just been told was down.
//
// (2) is now the single TypeScript decider and (3)/(4) are gone. These guard that.
describe('every surface reads the same ladder', () => {
  const picker = readFileSync(`${new URL('..', import.meta.url).pathname}/components/LlmProviderPicker.tsx`, 'utf8')
  const providers = readFileSync(`${new URL('..', import.meta.url).pathname}/components/CustomProviders.tsx`, 'utf8')

  it('the chooser derives its tiles from phaseFor, never from its own ternary', () => {
    expect(picker).toContain('phaseFor(')
    // The shapes of the old local implementation. Any of them coming back is a second answer.
    expect(picker).not.toContain('last_test?.outcome.status ===')
    expect(picker).not.toContain("label: 'NOT DETECTED'")
    expect(picker).not.toContain("label: 'ERROR'")
  })

  it('the cloud endpoint is judged like every other provider, not by being selected', () => {
    // `value === 'custom'` may decide what is SELECTED. It may never decide what is LIVE.
    expect(picker).toContain('const customPhase = phaseFor(')
    expect(picker).toContain('live={!TILE_NOT_LIVE.includes(customPhase.kind)}')
    // …and the endpoint card takes that verdict rather than re-deriving one.
    expect(providers).toContain('live: boolean')
    expect(providers).toContain("{live ? 'In use' : 'Not connected'}")
  })

  it('a throttled provider stays live - the same call Rust makes', () => {
    // `classify_provider_health` keeps ok: true and sets rate_limited. A tile that went grey
    // here would contradict the banner about the same provider at the same moment.
    expect(picker).toContain("const TILE_NOT_LIVE: Phase['kind'][] = ['not_installed', 'failed']")
  })
})
