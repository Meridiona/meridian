//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The task composer's state machine: note → AI draft → review/edit → create.
//
// STATE LIVES IN A MODULE-LEVEL STORE, not in the component. Drafting is an LLM call
// that can take most of a minute, and the whole composer lives inside the Plan modal —
// which the user can close (Escape, backdrop, the X) while it runs. Component state
// would take the in-flight request AND the note they typed with it. Keeping it out
// here means closing and reopening picks the flow back up exactly where it was. Same
// reasoning, and the same shape, as `timeline/useWorklog.ts` and `planStore.ts`.
//
// The composer is a singleton — you write one task at a time — so unlike planStore
// (keyed by day) there is one entry, reset explicitly when a create succeeds.
//
// THE INVARIANT THIS FILE PROTECTS: the AI can never block a create. `draft` is
// additive — it fills `title`/`description` if it can, and sets `error` if it can't.
// The fields are always editable and `canCreate` only ever looks at the title. A cold
// model is a slow suggestion, never a dead end.
//
// All I/O goes through the Tauri bridge:
//   - draft  → invoke (flat args)   the LLM call (can take ~a minute)
//   - create → mutate (body)        creates the task + adds it to the day's plan
//   - edit   → mutate (body)        rewrites a task, wherever it lives

'use client'

import { useSyncExternalStore } from 'react'
import { invoke, mutate } from '@/lib/bridge'
import { refreshPlan } from '@/components/plan/planStore'
import { LOCAL_PROVIDER } from '@/lib/api-types'
import type {
  PlanTaskDraft, CreatedTask, EditPlanTaskResult,
} from '@/lib/api-types'

/** Vestigial route label the bridge takes (Tauri-only now). */
const API = '/api/plan/task'

/** Where the composer is right now. */
export type ComposerPhase = 'idle' | 'drafting' | 'creating'

export interface ComposerState {
  /** The rough note the user typed for the AI to shape. */
  note: string
  title: string
  description: string
  issueType: string
  /** `'local'` = personal, or a provider id to file a real ticket. */
  target: string
  /** The provider the "on your board" option is currently pointed at, remembered
   *  across a trip through Personal so flipping back doesn't silently re-pick the
   *  first tracker. Null until the user has chosen one. Never `'local'`. */
  boardProvider: string | null
  phase: ComposerPhase
  /** True once a draft landed and the user hasn't edited the field yet - drives the
   *  small "Drafted" chip. Cleared per-field on first keystroke. */
  titleDrafted: boolean
  descriptionDrafted: boolean
  /** A hard failure that stopped a create. */
  error: string | null
  /** WHICH step produced `error`. The composer reacts to the two very differently:
   *  a failed DRAFT is usually "there is no AI to draft with", which has a fix the
   *  user can be walked to, while a failed CREATE is about the task or the tracker
   *  and has nothing to do with the model. Without this the composer had to guess
   *  from the error text, and guessed wrong in both directions - offering to connect
   *  a provider after a Jira permissions failure, and offering nothing at all after
   *  a draft died on a signed-out CLI. */
  errorSource: 'draft' | 'create' | null
  /** The draft failed because the ENGINE failed, straight from the draft call.
   *
   *  This exists because health cannot answer it. `llm_provider_ok` scores the last
   *  RECORDED test, so a provider that connected fine a minute ago still reads healthy
   *  while every real call is failing - and the composer, which asked health, offered a
   *  Try again that failed identically every time with no route to the picker. The call
   *  that just failed is the only witness to that, so it is the one that says so. */
  providerDown: boolean
  /** A soft caveat from a successful create (e.g. filed but not yet on your board). */
  note_after: string | null
  /** Set when a create succeeded - the caller closes the composer and refreshes. */
  created: CreatedTask | null
}

const EMPTY: ComposerState = Object.freeze({
  note: '',
  title: '',
  description: '',
  issueType: 'Task',
  // Personal is the DEFAULT even with a tracker connected: filing a real ticket on a
  // shared team board is outward-facing and hard to undo, so it must be a deliberate
  // click rather than what a mis-tap does on a morning planning screen.
  target: LOCAL_PROVIDER,
  boardProvider: null,
  phase: 'idle' as ComposerPhase,
  titleDrafted: false,
  descriptionDrafted: false,
  error: null,
  errorSource: null,
  providerDown: false,
  note_after: null,
  created: null,
})

let state: ComposerState = EMPTY
const listeners = new Set<() => void>()

function patch(next: Partial<ComposerState>) {
  state = { ...state, ...next }
  listeners.forEach((l) => l())
}

function subscribe(l: () => void): () => void {
  listeners.add(l)
  return () => {
    listeners.delete(l)
  }
}

const errMsg = (e: unknown): string =>
  e instanceof Error ? e.message : typeof e === 'string' ? e : 'Something went wrong'

// ── Field edits ───────────────────────────────────────────────────────────────

export const setNote = (note: string) => patch({ note })
/** Editing a drafted field clears its chip - it's the user's words now. */
export const setTitle = (title: string) => patch({ title, titleDrafted: false })
export const setDescription = (description: string) =>
  patch({ description, descriptionDrafted: false })
export const setIssueType = (issueType: string) => patch({ issueType })
/** Point the composer at Personal or at one specific provider.
 *
 *  Picking a provider also remembers it as the board choice, so the "on your board"
 *  option stays on the tracker the user last chose rather than snapping back to
 *  whichever tracker happens to sort first. */
export const setTarget = (target: string) =>
  patch(target === LOCAL_PROVIDER ? { target } : { target, boardProvider: target })

/** Which provider the board option represents right now, given the trackers that are
 *  actually connected.
 *
 *  Falls back to the first connected tracker when nothing has been chosen yet, and ALSO
 *  when the remembered choice is no longer connected - a tracker can be disconnected in
 *  Settings while the composer sits open, and filing at a dead provider would fail at
 *  the daemon with nothing the user could have seen coming. Returns null only when there
 *  is no tracker at all, in which case no board option is offered. */
export const boardProviderFor = (s: ComposerState, trackerIds: string[]): string | null => {
  if (trackerIds.length === 0) return null
  if (s.boardProvider && trackerIds.includes(s.boardProvider)) return s.boardProvider
  return trackerIds[0]
}

/** True when the composer will file a real ticket (as opposed to keeping it personal). */
export const isBoardTarget = (s: ComposerState): boolean => s.target !== LOCAL_PROVIDER

/** Wipe the composer back to a blank slate (on open, and after a successful create). */
export const resetComposer = () => {
  state = EMPTY
  listeners.forEach((l) => l())
}

// ── Resuming after a detour ───────────────────────────────────────────────────
//
// `resetComposer` runs on every mount, which is right: a half-typed task from an hour ago
// is never what the user wants back. But there is exactly one case where the composer is
// remounted MID-FLOW rather than reopened - pressing "Draft with AI" with no provider
// connected sends the user to Settings, which replaces the plan modal in the shell's single
// modal slot, and the composer unmounts with the note they had just typed.
//
// The module store survives that unmount. The reset did not, so the note came back blank
// and the tour told them it was still there. This flag is how the two are told apart: the
// connect flow arms it before reopening the planner, and the composer consumes it once.
//
// Deliberately NOT part of ComposerState - it is a one-shot instruction to the next mount,
// not a field anything renders, and putting it in the rendered state invites a component to
// branch on "are we resuming" long after the answer stopped being true.
let resumePending = false

/** Tell the next composer mount to KEEP what is in the store and pick the flow back up.
 *  Call it before reopening the planner - see `TaskComposer`'s mount effect. */
export const armResume = () => { resumePending = true }

/** Read and clear the resume flag. One-shot by construction: a second mount after the same
 *  detour is a fresh open and must reset like any other. */
export const consumeResume = (): boolean => {
  const was = resumePending
  resumePending = false
  return was
}

/** Fewest words a title may have. A short title ("Roadmap", "Login bug") names a
 *  SUBJECT and omits the work - it's a label, and on a board tomorrow it tells you
 *  nothing you didn't already know. The same floor is written into the draft prompt
 *  (`assets/prompts/plan-task-draft.md`), so the AI and the manual path agree; no
 *  tracker enforces this for us (Jira caps summaries at 255 chars, GitHub at 256, and
 *  neither has a minimum), which is exactly why it lives here. */
export const MIN_TITLE_WORDS = 4

/** Words in a title, whitespace-separated. */
export const titleWords = (title: string): number =>
  title.trim().split(/\s+/).filter(Boolean).length

/** True when Create should be enabled: a title of at least [`MIN_TITLE_WORDS`] words
 *  AND a description. Both are required however the fields were filled - by hand or by
 *  the model - because a task with no description is the one you can't decode next week.
 *
 *  This still does not let the AI block a create: every field it fills is typeable by
 *  hand in the same box, so a dead model costs you typing, never the task. */
export const canCreate = (s: ComposerState): boolean =>
  titleWords(s.title) >= MIN_TITLE_WORDS &&
  s.description.trim().length > 0 &&
  s.phase === 'idle'

// ── The flow ──────────────────────────────────────────────────────────────────

/** Shape the note into title/description/type with the user's LLM.
 *
 *  Additive by design: on any failure it sets `error` and leaves the fields alone, so
 *  the user can just type. Ignores a second call while one is running (double-submit
 *  guard #2 - the button's `disabled` is #1, and this one survives an unmount). */
export function draftFromNote() {
  if (state.phase !== 'idle') return
  const note = state.note.trim()
  if (!note) return
  patch({ phase: 'drafting', error: null, errorSource: null, providerDown: false })
  invoke<PlanTaskDraft>('draft_plan_task', { note })
    .then((d) => {
      if (d.error) {
        // A soft failure: no fields, but the user is not blocked.
        patch({ phase: 'idle', error: d.error, errorSource: 'draft', providerDown: !!d.provider_down })
        return
      }
      patch({
        phase: 'idle',
        title: d.title,
        description: d.description,
        issueType: d.issue_type || 'Task',
        titleDrafted: !!d.title,
        descriptionDrafted: !!d.description,
        error: null,
        errorSource: null,
        providerDown: false,
      })
    })
    // The command itself threw - the CLI could not be run or its answer was not JSON.
    // Not attributable to the engine, so it stays a retryable error rather than sending
    // the user to reconfigure a provider that was never reached.
    .catch((e) => patch({ phase: 'idle', error: errMsg(e), errorSource: 'draft', providerDown: false }))
}

/** Create the task and add it to `day`'s plan, then refresh the shared plan store so
 *  the Today column and the timeline's "Today's focus" both reflect it at once.
 *
 *  On success sets `created` (the caller closes the composer). On failure sets `error`
 *  and keeps every field, so the user can retry - or flip to Personal - without
 *  retyping. */
export function createTask(day: string) {
  if (!canCreate(state)) return
  patch({ phase: 'creating', error: null, errorSource: null })
  mutate<CreatedTask>(API, 'create_plan_task', {
    title: state.title.trim(),
    description: state.description.trim(),
    issue_type: state.issueType,
    target: state.target,
    day,
  })
    .then((created) => {
      patch({ phase: 'idle', created, note_after: created.note })
      refreshPlan(day)
    })
    .catch((e) => patch({ phase: 'idle', error: errMsg(e), errorSource: 'create' }))
}

/** Rewrite a task's title/description, wherever it lives (a personal task is written
 *  directly; a board ticket goes to its tracker). One call either way - the routing is
 *  the daemon's job, not the UI's. Refreshes the plan so the card updates. */
export function editTask(
  day: string,
  taskKey: string,
  fields: { title?: string; description?: string },
): Promise<EditPlanTaskResult> {
  return mutate<EditPlanTaskResult>(API, 'edit_plan_task', {
    task_key: taskKey,
    ...fields,
  }).then((r) => {
    refreshPlan(day)
    return r
  })
}

// ── The hook: a thin view over the store ──────────────────────────────────────

/** Subscribe to the composer. Survives the Plan modal unmounting mid-draft. */
export function useTaskComposer(): ComposerState {
  return useSyncExternalStore(
    subscribe,
    () => state,
    () => EMPTY,
  )
}
