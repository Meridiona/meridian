//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// Owns the walkthrough: decides whether to run it, implements the `Stage` the
// script is written against, and renders both of its layers.
//
// The seam with the app is deliberately tiny. This hook returns two nodes the
// shell drops in (`screen`, `overlay`) and takes two setters so beats can open
// the REAL planner and Settings. Nothing else in the product knows the
// walkthrough exists — no `sample` props threaded through the timeline or the
// right panel, no demo branches inside components that run every day. The tour
// is a temporary surface, so it owns its own pixels and its own state.
//
// # Who calls this
// `MeridianTimelineShell`.
//
// # Related
// - `./script.ts` — the beats this executes
// - `./engine.ts` — `Stage`, `Aborted`, and the DOM helpers
// - `./TutorialScreen.tsx` — the surface it renders
// - `./sampleDay.ts` — the example day (timeline cards AND right-panel stats)

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { SettingsSection } from '@/components/timeline/settings/types'
import type { DayTaskDetail } from '@/components/timeline/DayTaskDetailPanel'
import { load } from '@/lib/bridge'
import { connectedTrackers } from '@/lib/integrations'
import type { IntegrationsResponse } from '@/lib/api-types'
import {
  Aborted, setTutorialRunning, sleep, tourTarget, tween, waitForElement,
  type GhostCard, type Stage, type StageChoice, type StageModal,
} from './engine'
import { runScript } from './script'
import { sampleOverview, sampleTasks } from './sampleDay'
import { TutorialIntro } from './TutorialIntro'
import { TutorialOverlay } from './TutorialOverlay'
import { TutorialScreen } from './TutorialScreen'

/** Marker key for "this user has seen the walkthrough".
 *
 *  localStorage rather than a settings.json/DB flag on purpose: the walkthrough
 *  is a property of this install's UI, it must be readable synchronously on
 *  first paint (an async read would flash the dashboard before the walkthrough
 *  could claim it), and the failure mode of losing it is a repeated walkthrough,
 *  not lost user data. */
const SEEN_KEY = 'meridian.walkthrough.seen.v1'

/** DEV ONLY: set to `'day'` to start the walkthrough at part two. Read by
 *  `runScript`'s `devDayHalfOnly`, which is itself gated on a non-production
 *  build - so this key does nothing in a packaged app. Kept in localStorage
 *  rather than passed as an argument so it survives the reload a `?tour=day` URL
 *  forces, and so the console entry point works without one. */
const DEV_FROM_KEY = 'meridian.walkthrough.dev-from'

/** True in a dev build, where the shortcut into part two is offered. */
export const TOUR_DEV_TOOLS = process.env.NODE_ENV !== 'production'

// Built once at module load: the example day is static, and rebuilding it per
// render would hand `DayTaskColumn` a new array identity every frame.
const SAMPLE_TASKS = sampleTasks()
const SAMPLE_OVERVIEW = sampleOverview()
const NO_TASKS: typeof SAMPLE_TASKS = []

export interface TutorialHandle {
  running: boolean
  /** The opaque tutorial surface (an example day), or null. Rendered by the
   *  shell BELOW its modals, so beats that hand over the real planner/Settings
   *  show them on top. */
  screen: React.ReactNode
  /** Cursor, spotlight, narration and Skip. Sits above everything. */
  overlay: React.ReactNode
  /** Restart from the top. Surfaced in Settings → Account so it is reachable
   *  without clearing a marker from the console. */
  replay: () => void
  /** DEV ONLY: restart straight into part two - the example day rebuilding, the
   *  worklog flow, the summary. Reaching that half honestly costs a plan, an
   *  OAuth round-trip and a provider install, which is how a walkthrough stops
   *  being edited. `null` in a production build, and the control that offers it
   *  is not rendered. */
  replayFromDay: (() => void) | null
}

export function useTutorial(opts: {
  /** Opens a real modal — the only way the walkthrough touches the product. */
  setActiveModal: (m: StageModal) => void
  setSettingsSection: (s: SettingsSection) => void
  /** Make the next Settings open a REQUIRED step. See `Stage.openSettings`. */
  setSettingsLock: (l: 'soft' | 'required' | undefined) => void
  /** Raise the shell's REAL end-of-day scheduling dialog. Owned by the shell
   *  because it is a product surface with its own trigger and its own persisted
   *  flag; the tour only decides WHEN, in the one beat where the question makes
   *  sense. */
  setShowWorklogSchedule: (on: boolean) => void
  /** False until the shell has its data — the walkthrough should not take over
   *  a still-loading dashboard. */
  ready: boolean
}): TutorialHandle {
  const { setActiveModal, setSettingsSection, setSettingsLock, setShowWorklogSchedule, ready } = opts

  const [running, setRunning] = useState(false)
  const [caption, setCaption] = useState('')
  const [centered, setCentered] = useState(false)
  const [cursorAt, setCursorAt] = useState<string | null>(null)
  // Overrides `cursorAt` while a demonstration drag is in flight: the cursor has
  // to ride the card to an arbitrary POINT inside the drop zone, and every other
  // beat aims it at the centre of an element.
  const [cursorPoint, setCursorPoint] = useState<{ x: number; y: number } | null>(null)
  const [ghost, setGhost] = useState<GhostCard | null>(null)
  const [clicking, setClicking] = useState(false)
  const [spotlight, setSpotlight] = useState<string | null>(null)
  const [spotlightDim, setSpotlightDim] = useState(false)
  const [awaiting, setAwaiting] = useState(false)
  const [choices, setChoices] = useState<StageChoice[] | null>(null)
  // The surface's own state — all of it lives here rather than in the shell,
  // which is what keeps the shell free of walkthrough concerns.
  const [example, setExample] = useState(false)
  // Minutes past 9 AM while the day replay runs, `null` the rest of the time.
  // Drives the clock AND how much of the example day is on screen, so the two
  // cannot disagree about what time it is.
  const [replayMin, setReplayMin] = useState<number | null>(null)
  /** The current `replayDay` narration mark, rendered in the right panel rather
   *  than the overlay bar - see `replayDay` for why. */
  const [replayNote, setReplayNote] = useState<string | null>(null)
  const [captionBig, setCaptionBig] = useState(false)
  const [celebrate, setCelebrate] = useState(false)
  const [selected, setSelected] = useState<DayTaskDetail | null>(null)
  const [summaryOpen, setSummaryOpen] = useState(false)
  // Which tracker the user actually connected. Read once when the summary beats
  // are about to run rather than passed down from the shell: the connect happens
  // DURING the tour, so a value threaded in at mount would still be null by the
  // time the post beat needs to name their board.
  const [provider, setProvider] = useState<{ id: string; name: string } | null>(null)
  // Non-null only while the title sequence is playing.
  const [intro, setIntro] = useState<string[] | null>(null)

  const abortRef = useRef<AbortController | null>(null)
  // The click a beat is currently waiting on. A ref (not state) because the
  // document listener below must read the LATEST value without being torn down
  // and rebuilt on every beat.
  const pendingRef = useRef<{ selector: string; resolve: (clicked: boolean) => void } | null>(null)
  // Resolver for an open `ask`/`next`, settled from outside the render that
  // created it — same reasoning as `pendingRef`.
  const choiceRef = useRef<((v: string | null) => void) | null>(null)
  // Resolver for the title sequence, settled by the component when it finishes.
  const introRef = useRef<(() => void) | null>(null)

  const finish = useCallback(() => {
    abortRef.current?.abort()
    pendingRef.current?.resolve(false)
    pendingRef.current = null
    choiceRef.current?.(null)
    choiceRef.current = null
    introRef.current?.()
    introRef.current = null
    setReplayNote(null)
    setIntro(null)
    setRunning(false)
    setTutorialRunning(false)
    setCaption('')
    setCentered(false)
    setCursorAt(null)
    setCursorPoint(null)
    setGhost(null)
    setSpotlight(null)
    setSpotlightDim(false)
    setAwaiting(false)
    setChoices(null)
    setExample(false)
    setReplayMin(null)
    setCaptionBig(false)
    setCelebrate(false)
    setSelected(null)
    setSummaryOpen(false)
    setActiveModal(null)
    setShowWorklogSchedule(false)
    try { localStorage.setItem(SEEN_KEY, new Date().toISOString()) } catch { /* private mode */ }
  }, [setActiveModal])

  // A real click on the awaited target resolves the beat. Capture phase so the
  // walkthrough sees it even if the target stops propagation (day-task cards
  // do exactly that), and it never swallows the event — the app still handles
  // the click normally, which is the point of operating real controls.
  useEffect(() => {
    if (!running) return
    const onClick = (e: MouseEvent) => {
      const p = pendingRef.current
      if (!p) return
      const target = e.target as Element | null
      if (target?.closest(p.selector)) {
        pendingRef.current = null
        p.resolve(true)
      }
    }
    document.addEventListener('click', onClick, true)
    return () => document.removeEventListener('click', onClick, true)
  }, [running])

  const stage: Stage = useMemo(() => {
    const signal = () => {
      const s = abortRef.current?.signal
      if (!s || s.aborted) throw new Aborted()
      return s
    }

    // THE CURSOR RETRACTS. It exists to say "click HERE", so the instant that is
    // no longer true it has to go - otherwise it stays parked on a control the beat
    // has finished with and then glides across the screen toward it during the NEXT
    // beat, chasing a target nobody is being asked to press. That is what left a
    // cursor drifting at the plan nudge while the connect modal was open, and it is
    // a whole class of bug rather than one site: every interaction end and every
    // change of surface needs it, so they all funnel through here.
    const parkCursor = () => { setCursorAt(null); setCursorPoint(null) }

    // The press/release pulse the cursor draws where it currently sits.
    const pulse = () => {
      setClicking(true)
      setTimeout(() => setClicking(false), 200)
    }

    // Shared by `ask` and `next` — a Next button is an `ask` with one answer.
    const stageAsk = async (question: string, options: StageChoice[], center = false, party = false) => {
      const sig = signal()
      // A question is addressed to the user, not to anything on screen.
      parkCursor()
      setCaption(question)
      setCentered(center)
      setCelebrate(party)
      setChoices(options)
      const value = await new Promise<string | null>((resolve) => {
        const settle = (v: string | null) => {
          choiceRef.current = null
          sig.removeEventListener('abort', onAbort)
          resolve(v)
        }
        function onAbort() { settle(null) }
        sig.addEventListener('abort', onAbort, { once: true })
        choiceRef.current = settle
      })
      setChoices(null)
      setCentered(false)
      setCelebrate(false)
      if (sig.aborted) throw new Aborted()
      return value
    }

    return {
      get aborted() { return abortRef.current?.signal.aborted ?? true },
      say: (t) => { setCaption(t); setCentered(false) },
      spotlight: (sel, o) => {
        setSpotlight(sel)
        // Clearing the spotlight always clears the dim with it — a blur left
        // fenced around nothing would lock the whole window.
        setSpotlightDim(sel ? !!o?.dim : false)
      },
      openModal: (m) => { parkCursor(); setActiveModal(m) },
      openSettings: (section, o) => {
        // The surface is changing wholesale; whatever was being pointed at is
        // about to be covered or unmounted.
        parkCursor()
        setSettingsSection(section)
        // Set BEFORE the modal opens: SettingsModal reads `lock` on its first
        // render to decide whether the step is required and whether the picker
        // opens on its gate question, and a lock applied a frame later would
        // flash the unlocked screen first.
        setSettingsLock(o?.lock ? 'required' : undefined)
        setActiveModal('settings')
      },
      demoSummary: (on) => {
        // Read the connected tracker as the summary opens, not at mount: the
        // connect happened DURING this run, so anything captured earlier is
        // still null when the post beat needs to name their board. Failure is
        // silent and falls back to "your board", which is true either way.
        if (on) {
          load<IntegrationsResponse>('/api/integrations', 'get_integrations')
            .then(r => {
              const t = connectedTrackers(r)[0]
              setProvider(t ? { id: t.id, name: t.name } : null)
            })
            .catch(() => setProvider(null))
        }
        setSummaryOpen(on)
      },
      showWorklogSchedule: (on) => { parkCursor(); setShowWorklogSchedule(on) },
      showExample: (on) => setExample(on),
      selectTask: (id) => {
        if (id === null) { setSelected(null); return }
        // A real click, so `DayTaskColumn` builds the layout-derived
        // `DayTaskDetail` rather than this forking its maths — see Stage's doc.
        // Surface-scoped: the live dashboard behind the replica has cards with
        // the same ids, and clicking one of THOSE selects a task on a screen the
        // user cannot see.
        ;(tourTarget(`[data-task-id="${id}"]`) as HTMLElement | null)?.click()
      },
      intro: async (lines) => {
        const sig = signal()
        setIntro(lines)
        await new Promise<void>((resolve) => {
          const settle = () => {
            introRef.current = null
            sig.removeEventListener('abort', settle)
            resolve()
          }
          sig.addEventListener('abort', settle, { once: true })
          introRef.current = settle
        })
        setIntro(null)
        if (sig.aborted) throw new Aborted()
      },
      next: async (text, label = 'Next', o) => { await stageAsk(text, [{ label, value: 'next' }], o?.center, o?.celebrate) },
      replayDay: async (marks) => {
        const sig = signal()
        const HOURS = 9              // 9 AM → 6 PM
        const PER_HOUR_MS = 900      // the sweep itself
        const SETTLE_MS = 340        // a beat on the hour, so each one registers

        // THE NARRATION GOES IN THE RIGHT PANEL, not the overlay bar.
        //
        // It used to run in the bar at headline size, which is centred at the top
        // of the window - directly on top of the first two hours of the timeline.
        // So the one stretch of the tour the user is meant to WATCH had its own
        // commentary covering the thing being watched, and the bigger the type
        // the more of the day it hid. The right panel is the replay's own column
        // and is otherwise nearly empty, so the words and the work stop fighting
        // for the same pixels. `ReplayAside` renders it.
        setCaption('')
        setCaptionBig(false)
        setReplayMin(0)
        await sleep(800, sig)

        for (let h = 0; h < HOURS; h++) {
          const mark = marks.find(m => m.hour === 9 + h)
          if (mark) setReplayNote(mark.say)
          await tween(h * 60, (h + 1) * 60, PER_HOUR_MS, sig, setReplayMin)
          await sleep(SETTLE_MS, sig)
        }
        // The last mark (6 PM) lands on a finished board rather than being read
        // over the final hour still filling.
        const last = marks.find(m => m.hour === 9 + HOURS)
        if (last) setReplayNote(last.say)
        await sleep(900, sig)
        // Clock away, day left whole. `replayMin` going null is what hands the
        // right panel back to the example overview - which is the payoff shot,
        // and is also what the beats after this one point at.
        setReplayMin(null)
        setReplayNote(null)
        await sleep(600, sig)
        if (sig.aborted) throw new Aborted()
      },
      // ALWAYS centred. A question is not narration about something on screen -
      // it is the tour stopping and handing the user a decision, and the answer
      // changes which half of the walkthrough they get. Rendered in the top bar
      // it sat beside a live planner the user could still poke at, competing with
      // it for attention and reading as a caption on the page rather than as the
      // thing being asked of them. `next` keeps its opt-in `center`, because a
      // one-button "carry on" genuinely is narration.
      ask: (q, options) => stageAsk(q, options, true),
      waitForValue: async (sel, o) => {
        const sig = signal()
        const el = await waitForElement(sel, sig)
        if (!el) return false
        setAwaiting(true)
        const SETTLE_MS = 700
        const got = await new Promise<boolean>((resolve) => {
          let done = false
          let steadySince = 0
          let last = ''
          const settle = (v: boolean) => {
            if (done) return
            done = true
            sig.removeEventListener('abort', onAbort)
            clearTimeout(timer)
            cancelAnimationFrame(raf)
            resolve(v)
          }
          function onAbort() { settle(false) }
          const timer = setTimeout(() => settle(false), o?.fallbackMs ?? 120000)
          // Polled on frames rather than listening for `input`: React-controlled
          // fields and programmatic fills (the AI draft) do not both reliably
          // surface as the same event, and a missed event here reads to the user
          // as the walkthrough having frozen.
          let raf = requestAnimationFrame(function tick() {
            const node = tourTarget(sel) as HTMLInputElement | HTMLTextAreaElement | null
            // Target gone means the step moved on without us (the composer
            // swapped views); treat it as done rather than waiting out the
            // fallback behind a form that is no longer there.
            if (!node) { settle(true); return }
            const v = node.value.trim()
            if (v) {
              if (!o?.settled) { settle(true); return }
              if (v !== last) { last = v; steadySince = performance.now() }
              else if (performance.now() - steadySince > SETTLE_MS) { settle(true); return }
            }
            raf = requestAnimationFrame(tick)
          })
          sig.addEventListener('abort', onAbort, { once: true })
        })
        setAwaiting(false)
        parkCursor()
        if (sig.aborted) throw new Aborted()
        return got
      },
      value: (sel) => (tourTarget(sel) as HTMLInputElement | HTMLTextAreaElement | null)?.value ?? '',
      waitForMinWords: async (sel, minWords, o) => {
        const sig = signal()
        const el = await waitForElement(sel, sig)
        if (!el) return false
        const words = (v: string) => v.trim().split(/\s+/).filter(Boolean).length
        setAwaiting(true)
        const got = await new Promise<boolean>((resolve) => {
          let done = false
          const settle = (v: boolean) => {
            if (done) return
            done = true
            sig.removeEventListener('abort', onAbort)
            clearTimeout(timer)
            cancelAnimationFrame(raf)
            resolve(v)
          }
          function onAbort() { settle(false) }
          const timer = setTimeout(() => settle(false), o?.fallbackMs ?? 90000)
          // Same target-vanished handling as waitForValue/waitForClick: the
          // composer closing out from under this wait is the step ending, not
          // a miss, so it counts as done rather than burning the fallback.
          let raf = requestAnimationFrame(function tick() {
            const node = tourTarget(sel) as HTMLInputElement | HTMLTextAreaElement | null
            if (!node) { settle(true); return }
            if (words(node.value) >= minWords) { settle(true); return }
            raf = requestAnimationFrame(tick)
          })
          sig.addEventListener('abort', onAbort, { once: true })
        })
        setAwaiting(false)
        parkCursor()
        if (sig.aborted) throw new Aborted()
        return got
      },
      appeared: async (sel, timeoutMs = 3000) => !!(await waitForElement(sel, signal(), timeoutMs)),
      demoDrag: async (from, to) => {
        const sig = signal()
        const src = await waitForElement(from, sig)
        const dst = await waitForElement(to, sig)
        if (!src || !dst) return false
        const sr = src.getBoundingClientRect()
        const dr = dst.getBoundingClientRect()
        if (!sr.width || !dr.width) return false

        // How the real move actually happens, found BEFORE the animation runs so
        // a card with no add control never gets dragged to nowhere. This is the
        // same "+ Add" button the user can press themselves - the tour operates
        // the product's own controls rather than reaching into its state, which
        // is what keeps the demonstration honest and keeps this from drifting
        // when the add path changes.
        const add = src.querySelector<HTMLElement>('button[aria-label^="Add "]')
        if (!add) return false

        // 1. The hand arrives on the card and presses. Aimed at the card's LEFT
        //    edge, where the grip handle is - the part you would actually grab.
        const grabX = sr.left + Math.min(28, sr.width / 2)
        const grabY = sr.top + sr.height / 2
        setCursorAt(null)
        setCursorPoint({ x: grabX, y: grabY })
        await sleep(1000, sig)
        pulse()
        await sleep(280, sig)

        // 2. It lifts. Two commits, because a CSS transition needs a frame at the
        //    start value before the end value arrives - set together, the ghost
        //    would simply appear at the destination.
        setGhost({ html: src.outerHTML, left: sr.left, top: sr.top, width: sr.width, height: sr.height, dx: 0, dy: 0 })
        await sleep(220, sig)

        // 3. It travels, and the cursor rides with it. Both animate over the same
        //    1s with the same easing (see TutorialOverlay), so the card stays
        //    under the pointer for the whole trip instead of racing it.
        //    Landing point: just inside the top of the drop zone, which is where
        //    the first card in an empty Today column actually sits.
        const dropX = dr.left + dr.width / 2
        const dropY = dr.top + Math.min(44, dr.height / 2)
        setCursorPoint({ x: dropX, y: dropY })
        setGhost(g => g && { ...g, dx: dropX - (sr.left + sr.width / 2), dy: dropY - grabY })
        await sleep(1000, sig)

        // 4. Release, and let go for real. The click runs BEFORE the ghost is
        //    cleared so the card is already in Today as the ghost vanishes - the
        //    other order shows an empty column for a frame and reads as the drop
        //    having missed.
        pulse()
        await sleep(160, sig)
        add.click()
        await sleep(120, sig)
        setGhost(null)
        parkCursor()
        if (sig.aborted) throw new Aborted()
        return true
      },
      pause: (ms) => sleep(ms, signal()),
      point: async (sel) => {
        const sig = signal()
        // Beats point at things the previous beat just caused to render, so
        // querying immediately would race React's commit.
        const el = await waitForElement(sel, sig)
        if (!el) return
        setCursorPoint(null)
        setCursorAt(sel)
        await sleep(900, sig)
      },
      click: pulse,
      waitForClick: async (sel, fallbackMs = 7000) => {
        const sig = signal()
        const el = await waitForElement(sel, sig)
        // Target never rendered — do not strand the user waiting for something
        // that cannot be clicked.
        if (!el) return false
        setAwaiting(true)
        const clicked = await new Promise<boolean>((resolve) => {
          let done = false
          const settle = (v: boolean) => {
            if (done) return
            done = true
            pendingRef.current = null
            sig.removeEventListener('abort', onAbort)
            clearTimeout(timer)
            cancelAnimationFrame(raf)
            resolve(v)
          }
          function onAbort() { settle(false) }
          const timer = setTimeout(() => settle(false), fallbackMs)
          // The target VANISHING also ends the wait, and counts as done rather
          // than timed out. Every long wait in the script is on a modal's close
          // button, and a modal has three other ways out — Escape, a backdrop
          // click, and a flow that closes itself. Without this the walkthrough
          // sits behind a modal the user already dismissed, silently, until a
          // multi-minute fallback expires. `waitForElement` above guarantees the
          // element existed when this started, so its absence is an action
          // rather than a race with React's first commit.
          let raf = requestAnimationFrame(function tick() {
            if (!tourTarget(sel)) { settle(true); return }
            raf = requestAnimationFrame(tick)
          })
          sig.addEventListener('abort', onAbort, { once: true })
          pendingRef.current = { selector: sel, resolve: settle }
        })
        setAwaiting(false)
        parkCursor()
        if (sig.aborted) throw new Aborted()
        return clicked
      },
    }
  }, [setActiveModal, setSettingsSection, setSettingsLock, setShowWorklogSchedule])

  // Start once, on the first ready render for a user who has not seen it.
  const startedRef = useRef(false)

  // Set by the three explicit entry points below, and read by the start effect to
  // skip the "is this install entitled to the walkthrough" check.
  //
  // That check exists for the AUTOMATIC start only. Someone who pressed "Show me
  // around" has asked for the tour in so many words, and an install that predates
  // the walkthrough is precisely the one most likely to press it — refusing them
  // would make the control dead on the only machines it matters on.
  const explicitRef = useRef(false)

  // The same fact as `explicitRef`, but readable during RENDER — a ref is not,
  // and the overlay needs it to decide whether to show the Skip control at all.
  //
  // Set in one place (the start effect, next to `setRunning(true)`) rather than
  // in each of the three entry points: that is the single moment the answer is
  // final, so the two can't disagree about the run that is actually starting.
  const [explicit, setExplicit] = useState(false)

  // Replay hook. The walkthrough is a once-ever surface gated on a marker, so
  // without this the only way to see it again is deleting that key from the
  // console — every iteration, for anyone building or QAing it. Three ways in:
  //
  //   Settings → Account → "Show me around"
  //   ?tour=1                 on the dashboard URL (survives the reload)
  //   window.__meridianTour() from the console (no reload)
  const [replayNonce, setReplayNonce] = useState(0)
  const replay = useCallback(() => {
    explicitRef.current = true
    try { localStorage.removeItem(SEEN_KEY) } catch { /* private mode */ }
    // Clear the dev jump too, so the ordinary Replay always means the WHOLE
    // walkthrough. Without this, one use of the shortcut latches the tour into
    // its second half for the rest of the install - and the flag lives in
    // localStorage precisely so it survives reloads, so it would not clear
    // itself either.
    try { localStorage.removeItem(DEV_FROM_KEY) } catch { /* private mode */ }
    startedRef.current = false
    setReplayNonce((n) => n + 1)
  }, [])

  const replayFromDay = useCallback(() => {
    explicitRef.current = true
    try { localStorage.setItem(DEV_FROM_KEY, 'day') } catch { /* private mode */ }
    try { localStorage.removeItem(SEEN_KEY) } catch { /* private mode */ }
    startedRef.current = false
    setReplayNonce((n) => n + 1)
  }, [])

  /** DEV ONLY: the overlay's "Skip to the day" pill - jump straight to part two
   *  (the timeline rebuilding itself) from wherever the tour currently is.
   *
   *  It is `finish` THEN `replayFromDay`, and the order is not incidental.
   *  `replayFromDay` alone only clears the started latch and bumps the nonce, so
   *  the effect starts a SECOND script while the first one is still sitting in an
   *  `await` - two scripts driving one stage, taking turns writing the caption.
   *  `finish` aborts the running one first (every beat throws `Aborted` out of
   *  its wait) and resets the surface; `replayFromDay` then clears the seen
   *  marker `finish` just wrote and restarts. */
  const skipToDay = useCallback(() => {
    finish()
    replayFromDay()
  }, [finish, replayFromDay])

  useEffect(() => {
    const w = window as unknown as {
      __meridianTour?: () => void
      __meridianTourDay?: () => void
    }
    w.__meridianTour = replay
    // Same jump as the Settings control, for when the console is closer to hand
    // than the modal - notably mid-tour, when Settings is being driven by a beat.
    w.__meridianTourDay = replayFromDay
    return () => { delete w.__meridianTour; delete w.__meridianTourDay }
  }, [replay, replayFromDay])

  // Declared BEFORE the start effect on purpose: effects run in declaration order,
  // so `explicitRef` is already set by the time the start effect reads it.
  useEffect(() => {
    if (!new URLSearchParams(window.location.search).has('tour')) return
    explicitRef.current = true
    try { localStorage.removeItem(SEEN_KEY) } catch { /* private mode */ }
  }, [])

  // `stage`/`finish` are read through refs and are NOT effect dependencies, and
  // the effect does NOT abort on cleanup. Both are load-bearing.
  //
  // The bug they fix: `stage` is memoised on the shell's setters, and the shell
  // re-renders constantly because useTimelineData polls. Any churn in those deps
  // re-ran the effect, whose cleanup aborted the AbortController out from under
  // the running script; the re-run then hit the `startedRef` guard and returned
  // without restarting. The walkthrough froze on whatever caption was on screen
  // — alive, `running` still true, but permanently unwound. It looked exactly
  // like a tutorial with no way to advance.
  const stageRef = useRef(stage)
  stageRef.current = stage
  const finishRef = useRef(finish)
  finishRef.current = finish

  // The only legitimate reason to abort is the component going away. A skip
  // aborts explicitly, via `finish`.
  useEffect(() => () => abortRef.current?.abort(), [])

  useEffect(() => {
    if (!ready || startedRef.current) return
    let seen = true
    try { seen = !!localStorage.getItem(SEEN_KEY) } catch { seen = true }
    if (seen) return
    startedRef.current = true

    const ctrl = new AbortController()
    abortRef.current = ctrl
    ;(async () => {
      if (ctrl.signal.aborted) return
      // THE UPGRADE GUARD. `SEEN_KEY` above answers "has this browser profile been
      // shown the walkthrough", and for every install that predates the walkthrough
      // the honest answer is no - so on its own it hands a full-screen tour of the
      // product to someone who has been using it for months, on the first launch
      // after an update. `walkthrough_is_armed` is the missing half: it is written
      // only by the wizard's own completion, so absent means "onboarded before this
      // existed" and the tour stays out of the way.
      //
      // Failure is treated as NOT armed. The cost of being wrong in that direction
      // is a walkthrough the user can still reach from Settings; the other way it is
      // an unskippable-looking takeover of a working dashboard.
      if (!explicitRef.current) {
        const armed = await load<boolean>('/api/walkthrough-armed', 'walkthrough_is_armed')
          .catch(() => false)
        if (!armed || ctrl.signal.aborted) return
      }
      // Captured at the one moment it is settled, and read by the overlay to
      // decide whether a Skip control exists for this run. See `explicit` above.
      setExplicit(explicitRef.current)
      setRunning(true)
      // Mirrored into the module flag so screens BELOW the shell can tell they are
      // being demonstrated rather than used - see `isTutorialRunning`.
      setTutorialRunning(true)
      try {
        await runScript(stageRef.current)
      } catch (e) {
        if (!(e instanceof Aborted)) throw e
        return
      }
      finishRef.current()
    })().catch(() => finishRef.current())
    // replayNonce re-runs this effect after `replay` clears the marker and the
    // started latch; it is otherwise unused.
  }, [ready, replayNonce])

  return {
    running,
    replay,
    replayFromDay: TOUR_DEV_TOOLS ? replayFromDay : null,
    screen: running ? (
      <TutorialScreen
        phase={example ? 'example' : 'empty'}
        // ALWAYS the whole day, replay or not - the reveal is a property of the
        // CARD (`replayNowMin`, below), never of this list.
        //
        // This used to hand down a filtered list that grew as the clock advanced,
        // and it was the reason the replay looked wrong. `DayTaskColumn` sizes
        // its vertical scale to fit the tasks it is given (`buildTimelineScale`
        // + `taskWindow`), so a growing list meant a re-scaling column: the first
        // card landed filling the pane, then every later one squeezed the whole
        // timeline to make room. That reads as the view zooming out between
        // cards, and the hour rail slid under the descending line instead of
        // staying put beneath it.
        //
        // The marketing demo never had this problem because it does the opposite
        // (demo.js `buildSkeleton`): it renders the FINISHED day first, marks
        // every band `is-pre`, and the sweep only takes that class off. The
        // geometry is settled before the first frame, so nothing moves but the
        // line and the cards' own opacity. That is what this now does.
        tasks={example ? SAMPLE_TASKS : NO_TASKS}
        replayMinute={replayMin}
        replayNote={replayNote}
        sample={SAMPLE_OVERVIEW}
        selected={selected}
        onSelect={setSelected}
        summaryOpen={summaryOpen}
        provider={provider}
        // A NO-OP during the tour, deliberately. This is the real product's
        // handler - click the plan nudge, the planner opens - but while the
        // walkthrough is running the SCRIPT owns which surface is up, and the
        // next thing it needs is the team-or-solo question with a clear screen
        // behind it. Letting the button through opened the planner for one beat
        // so the script could immediately close it again: a full modal flying in
        // and straight back out, reading as a misfire right where the tour is
        // asking the user to trust it. The click still registers - `waitForClick`
        // observes the event, it does not depend on the effect - and the script
        // opens the planner itself once the question is answered.
        onOpenPlan={() => {}}
      />
    ) : null,
    overlay: running ? (
      <>
        <TutorialOverlay
          caption={caption} centered={centered} big={captionBig} celebrate={celebrate}
          cursorAt={cursorAt} cursorPoint={cursorPoint}
          ghost={ghost} clicking={clicking}
          spotlight={spotlight} spotlightDim={spotlightDim} awaiting={awaiting}
          choices={choices} onChoose={(v) => choiceRef.current?.(v)}
          // NO SKIP ON THE FIRST RUN. `null` here removes the control entirely
          // rather than hiding or disabling it.
          //
          // The onboarding walkthrough is the one pass where the product gets to
          // explain itself, and it is three minutes long. An out placed in the
          // corner of the very first screen is read as the recommended action by
          // anyone who is unsure - which is everyone, at that exact moment - so
          // it was costing the explanation to the users who most needed it.
          //
          // A REPLAY keeps it. That run is one the user asked for out loud
          // (Settings, `?tour=1`, or the console hook), by someone who has
          // already seen the product, so trapping them for three minutes would
          // be a different bug and not the one being fixed here. `explicit` is
          // exactly that distinction, captured where the run starts.
          onSkip={explicit ? finish : null}
          // Null in a packaged build, so the pill is not rendered at all. This
          // jumps PAST the compulsory AI-connect step, which the whole product
          // needs - it is a shortcut for whoever is editing part two, not a way
          // out for a real first run.
          onSkipToDay={TOUR_DEV_TOOLS ? skipToDay : null}
        />
        {/* Above the overlay, opaque: the title sequence owns the screen
            outright, so nothing from the tour or the dashboard shows through
            underneath it while it plays. */}
        {intro && <TutorialIntro lines={intro} onDone={() => introRef.current?.()} />}
      </>
    ) : null,
  }
}
