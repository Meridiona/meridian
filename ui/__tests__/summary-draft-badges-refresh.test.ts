//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Guards that the daily summary's worklog badges are re-read after the user acts on
// one, rather than staying frozen at whatever they were when the overlay opened.
//
// THE BUG: `DaySummaryOverlay` read `get_day_draft_states` exactly once, inside the
// day-change effect, and never again for the life of the overlay. Every path that
// CHANGES a draft's state - approve, post, dismiss, retarget - lives in
// `SummaryTaskView`, which the summary opens over itself and then returns from. So
// the user opened a row, posted the update, came back, and the row they had just
// filed still read DRAFT READY TO POST, in the loud accent treatment reserved for
// "this needs you". Posting three rows left three rows still shouting.
//
// Measured on a real DB while it was happening: `day_task_worklogs` had flipped to
// `posted` for the rows in question. The write was never the problem - the screen
// simply never asked again. And because the tint and the `›` chevron key off the
// same badge, the whole list still looked like a queue of outstanding work.
//
// TWO EXIT PATHS, not one. The task view is left by its own Back button AND by
// Escape (handled in the overlay's own key listener, which nulls `selected`
// directly). A refresh hung off `onBack` alone would leave the Escape path exactly
// as broken as before, which is why both must go through one helper.
//
// No React render harness in this repo (see summary-plan-row-drafts / plan-store),
// so this is source-scanned: the assertions are about the wiring, which is the half
// that was wrong.

import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'fs'

const uiRoot = import.meta.dir + '/..'
const src = readFileSync(uiRoot + '/components/summary/DaySummaryOverlay.tsx', 'utf8')

/** The `refreshDrafts` definition - the single read site for the badge map. */
function refreshDrafts(): string {
  const at = src.indexOf('const refreshDrafts = useCallback(')
  expect(at).toBeGreaterThan(-1)
  const end = src.indexOf('\n  }, [', at)
  expect(end).toBeGreaterThan(at)
  return src.slice(at, end)
}

/** The `closeTask` definition - the single exit from the task view. */
function closeTask(): string {
  const at = src.indexOf('const closeTask = useCallback(')
  expect(at).toBeGreaterThan(-1)
  const end = src.indexOf('\n  }, [', at)
  expect(end).toBeGreaterThan(at)
  return src.slice(at, end)
}

/** The effect that tracks the visible day into `dayRef`. */
function dayTracker(): string {
  const at = src.indexOf('dayRef.current = day')
  expect(at).toBeGreaterThan(-1)
  const from = src.lastIndexOf('useEffect(', at)
  expect(from).toBeGreaterThan(-1)
  const end = src.indexOf('[day])', at)
  expect(end).toBeGreaterThan(at)
  return src.slice(from, end)
}

/** The Escape key handler. */
function escapeHandler(): string {
  const at = src.indexOf('function onKey(')
  expect(at).toBeGreaterThan(-1)
  const end = src.indexOf('\n    }', at)
  expect(end).toBeGreaterThan(at)
  return src.slice(at, end)
}

describe('the badge map is re-read after the user acts on a draft', () => {
  it('has one reusable read, not a read buried in the day effect', () => {
    // THE REGRESSION: `load(... 'get_day_draft_states' ...)` appeared once, inside
    // `Promise.allSettled` in the day-change effect, and nowhere else - so the only
    // way to refresh the badges was to close the overlay and reopen it.
    expect(refreshDrafts()).toContain("'get_day_draft_states'")
  })

  it('refuses to write one day\'s badges under another day\'s rows', () => {
    // A slow read landing after the user paged days would otherwise repaint the new
    // day's list with the old day's states. `dayRef` already exists for exactly this
    // (the compose paths use it); the refresh has to honour it too.
    expect(refreshDrafts()).toContain('dayRef.current')
  })

  it('refuses to let an older refresh land on top of a newer one', () => {
    // `dayRef` does NOT cover this: two refreshes for the SAME day can overlap (close
    // a task, open another, act on it, close it - all faster than one read resolving),
    // and nothing orders the responses. The older one landing last would restore
    // exactly the badges the newer one just corrected - this file's own bug, through
    // a different door.
    const body = refreshDrafts()
    expect(body).toContain('++draftRequest.current')
    expect(body).toContain('req !== draftRequest.current')
  })

  it('invalidates an in-flight refresh whenever the day changes', () => {
    // The hole the two guards above leave OPEN, together. Page A -> B -> A while a
    // refresh for A is still in flight and BOTH checks wave it through: `dayRef` is
    // back to A, and the request number is untouched because returning to A starts
    // the day effect's own read, not another `refreshDrafts`. So a response from
    // before the round trip lands on top of the fresh one the return to A just
    // painted - the same "older answer wins" failure as two overlapping refreshes,
    // reached by navigating instead of by clicking.
    //
    // Bumping the counter on every day transition closes it: the number a refresh
    // captured before the user left is no longer current when it returns, so the
    // response is dropped regardless of which day is showing.
    expect(dayTracker()).toContain('draftRequest.current')
  })

  it('keeps the badges it already has when a refresh read fails', () => {
    // The opposite of the first-paint rule, deliberately. On a refresh there is
    // already an answer on screen, so a failed read must leave it: blanking every
    // badge would tell the user their drafts had vanished. Only the day effect - the
    // FIRST answer for a day - is allowed to render badgeless.
    const body = refreshDrafts()
    expect(body).toContain('.catch(() => null)')
    expect(body).toContain('if (rows) setDrafts(')
  })

  it('is called when the task view is left', () => {
    expect(closeTask()).toContain('refreshDrafts()')
    expect(closeTask()).toContain('setSelected(null)')
  })

  it('routes the Back button through that same close', () => {
    const at = src.indexOf('<SummaryTaskView')
    expect(at).toBeGreaterThan(-1)
    const end = src.indexOf('/>', at)
    expect(end).toBeGreaterThan(at)
    expect(src.slice(at, end)).toContain('onBack={closeTask}')
  })

  it('routes Escape through it too, rather than nulling `selected` itself', () => {
    // The path that made a one-line `onBack` fix insufficient. Escape is a first-
    // class way out of the task view and has to refresh for the same reason Back
    // does - the user posted, then dismissed the view with the keyboard.
    const body = escapeHandler()
    expect(body).toContain('closeTask()')
    expect(body).not.toContain('setSelected(null)')
  })
})
