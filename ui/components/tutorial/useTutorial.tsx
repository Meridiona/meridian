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
import type { DayTasksResponse } from '@/lib/api-types'
import { Aborted, sleep, waitForElement, type Stage, type StageChoice, type StageModal } from './engine'
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
}

export function useTutorial(opts: {
  /** Opens a real modal — the only way the walkthrough touches the product. */
  setActiveModal: (m: StageModal) => void
  setSettingsSection: (s: SettingsSection) => void
  /** False until the shell has its data — the walkthrough should not take over
   *  a still-loading dashboard. */
  ready: boolean
}): TutorialHandle {
  const { setActiveModal, setSettingsSection, ready } = opts

  const [running, setRunning] = useState(false)
  const [caption, setCaption] = useState('')
  const [centered, setCentered] = useState(false)
  const [cursorAt, setCursorAt] = useState<string | null>(null)
  const [clicking, setClicking] = useState(false)
  const [spotlight, setSpotlight] = useState<string | null>(null)
  const [spotlightDim, setSpotlightDim] = useState(false)
  const [awaiting, setAwaiting] = useState(false)
  const [choices, setChoices] = useState<StageChoice[] | null>(null)
  // The surface's own state — all of it lives here rather than in the shell,
  // which is what keeps the shell free of walkthrough concerns.
  const [example, setExample] = useState(false)
  const [selected, setSelected] = useState<DayTaskDetail | null>(null)
  const [summaryOpen, setSummaryOpen] = useState(false)
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
    setIntro(null)
    setRunning(false)
    setCaption('')
    setCentered(false)
    setCursorAt(null)
    setSpotlight(null)
    setSpotlightDim(false)
    setAwaiting(false)
    setChoices(null)
    setExample(false)
    setSelected(null)
    setSummaryOpen(false)
    setActiveModal(null)
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

    // Shared by `ask` and `next` — a Next button is an `ask` with one answer.
    const stageAsk = async (question: string, options: StageChoice[], center = false) => {
      const sig = signal()
      setCaption(question)
      setCentered(center)
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
      openModal: (m) => setActiveModal(m),
      openSettings: (section) => { setSettingsSection(section); setActiveModal('settings') },
      demoSummary: (on) => setSummaryOpen(on),
      showExample: (on) => setExample(on),
      selectTask: (id) => {
        if (id === null) { setSelected(null); return }
        // A real click, so `DayTaskColumn` builds the layout-derived
        // `DayTaskDetail` rather than this forking its maths — see Stage's doc.
        document.querySelector<HTMLElement>(`[data-task-id="${id}"]`)?.click()
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
      next: async (text, label = 'Next', o) => { await stageAsk(text, [{ label, value: 'next' }], o?.center) },
      ask: (q, options) => stageAsk(q, options),
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
            const node = document.querySelector<HTMLInputElement | HTMLTextAreaElement>(sel)
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
        if (sig.aborted) throw new Aborted()
        return got
      },
      appeared: async (sel, timeoutMs = 3000) => !!(await waitForElement(sel, signal(), timeoutMs)),
      pause: (ms) => sleep(ms, signal()),
      point: async (sel) => {
        const sig = signal()
        // Beats point at things the previous beat just caused to render, so
        // querying immediately would race React's commit.
        const el = await waitForElement(sel, sig)
        if (!el) return
        setCursorAt(sel)
        await sleep(900, sig)
      },
      click: () => {
        setClicking(true)
        setTimeout(() => setClicking(false), 200)
      },
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
            if (!document.querySelector(sel)) { settle(true); return }
            raf = requestAnimationFrame(tick)
          })
          sig.addEventListener('abort', onAbort, { once: true })
          pendingRef.current = { selector: sel, resolve: settle }
        })
        setAwaiting(false)
        if (sig.aborted) throw new Aborted()
        return clicked
      },
    }
  }, [setActiveModal, setSettingsSection])

  // Start once, on the first ready render for a user who has not seen it.
  const startedRef = useRef(false)

  // Replay hook. The walkthrough is a once-ever surface gated on a marker, so
  // without this the only way to see it again is deleting that key from the
  // console — every iteration, for anyone building or QAing it. Three ways in:
  //
  //   Settings → Account → "Show me around"
  //   ?tour=1                 on the dashboard URL (survives the reload)
  //   window.__meridianTour() from the console (no reload)
  const [replayNonce, setReplayNonce] = useState(0)
  const replay = useCallback(() => {
    try { localStorage.removeItem(SEEN_KEY) } catch { /* private mode */ }
    startedRef.current = false
    setReplayNonce((n) => n + 1)
  }, [])

  useEffect(() => {
    const w = window as unknown as { __meridianTour?: () => void }
    w.__meridianTour = replay
    return () => { delete w.__meridianTour }
  }, [replay])

  useEffect(() => {
    if (!new URLSearchParams(window.location.search).has('tour')) return
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
    setRunning(true)
    ;(async () => {
      // Whether the user's OWN day already has folded tasks. One beat reads it,
      // to avoid telling someone with a full timeline that theirs is empty. A
      // failed read falls back to the empty wording, which is the first-run case
      // this exists for.
      let hasTasks = false
      try {
        const d = new Date()
        const p = (n: number) => String(n).padStart(2, '0')
        const today = `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`
        const resp = await load<DayTasksResponse>('/api/day-tasks', 'get_day_tasks', { day: today })
        hasTasks = (resp?.tasks?.length ?? 0) > 0
      } catch { hasTasks = false }
      if (ctrl.signal.aborted) return
      try {
        await runScript(stageRef.current, hasTasks)
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
    screen: running ? (
      <TutorialScreen
        phase={example ? 'example' : 'empty'}
        tasks={example ? SAMPLE_TASKS : NO_TASKS}
        sample={SAMPLE_OVERVIEW}
        selected={selected}
        onSelect={setSelected}
        summaryOpen={summaryOpen}
        onOpenPlan={() => setActiveModal('plan')}
      />
    ) : null,
    overlay: running ? (
      <>
        <TutorialOverlay
          caption={caption} centered={centered} cursorAt={cursorAt} clicking={clicking}
          spotlight={spotlight} spotlightDim={spotlightDim} awaiting={awaiting}
          choices={choices} onChoose={(v) => choiceRef.current?.(v)}
          onSkip={finish}
        />
        {/* Above the overlay, opaque: the title sequence owns the screen
            outright, so nothing from the tour or the dashboard shows through
            underneath it while it plays. */}
        {intro && <TutorialIntro lines={intro} onDone={() => introRef.current?.()} />}
      </>
    ) : null,
  }
}
