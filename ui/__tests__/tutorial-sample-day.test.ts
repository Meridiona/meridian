//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, it, expect } from 'bun:test'
import { readFileSync } from 'fs'
import {
  sampleTasks, sampleDayString, sampleOverview, FOCUS_TASK_ID, DRAFT_TASK_ID,
} from '../components/tutorial/sampleDay'
import { fmtSetupElapsed, INTRO_LINES } from '../components/tutorial/script'

// The example day is fed to the REAL DayTaskColumn via its `tasks` prop, so a
// shape mismatch does not fail loudly — it renders a subtly broken timeline in
// front of a brand-new user, during the one run where the product is trying to
// establish that it can be trusted to describe their work accurately. These
// tests exist because that failure is silent.

describe('sample day shape', () => {
  const tasks = sampleTasks()

  it('derives minutes from its own segments', () => {
    // Hand-written minutes would drift from the segments they claim to
    // summarise; the Rust fold computes this deterministically and so must this.
    for (const t of tasks) {
      const summed = t.segments.reduce((a, s) => {
        const [sh, sm] = s.start.split(':').map(Number)
        const [eh, em] = s.end.split(':').map(Number)
        return a + (eh * 60 + em) - (sh * 60 + sm)
      }, 0)
      expect(t.minutes).toBe(summed)
    }
  })

  it('emits ascending, well-formed HH:MM segments', () => {
    for (const t of tasks) {
      let prev = -1
      for (const s of t.segments) {
        expect(s.start).toMatch(/^\d{2}:\d{2}$/)
        expect(s.end).toMatch(/^\d{2}:\d{2}$/)
        const [sh, sm] = s.start.split(':').map(Number)
        const [eh, em] = s.end.split(':').map(Number)
        const start = sh * 60 + sm
        const end = eh * 60 + em
        expect(end).toBeGreaterThan(start)
        expect(start).toBeGreaterThan(prev)
        prev = end
      }
    }
  })

  it('labels hours as YYYY-MM-DDTHH matching first_hour/last_hour', () => {
    const day = sampleDayString()
    for (const t of tasks) {
      expect(t.hours.length).toBeGreaterThan(0)
      for (const h of t.hours) expect(h.startsWith(`${day}T`)).toBe(true)
      const nums = t.hours.map((h) => Number(h.slice(-2)))
      expect(t.first_hour).toBe(Math.min(...nums))
      expect(t.last_hour).toBe(Math.max(...nums))
    }
  })

  it('dates the example in the past, never today', () => {
    // The example runs to 17:20. Dated "today", a mid-afternoon install would
    // show the user hours of work they can see they have not done yet.
    const today = new Date()
    const p = (n: number) => String(n).padStart(2, '0')
    const todayStr = `${today.getFullYear()}-${p(today.getMonth() + 1)}-${p(today.getDate())}`
    expect(sampleDayString()).not.toBe(todayStr)
    expect(sampleDayString() < todayStr).toBe(true)
  })

  it('keeps the cards the script targets by id', () => {
    // The script points and waits on these specific ids; renaming or
    // reordering the list without updating them would leave beats pointing at
    // nothing, which degrades to a silent 6s stall rather than an error.
    expect(tasks.find((t) => t.id === FOCUS_TASK_ID)).toBeDefined()
    expect(tasks.find((t) => t.id === DRAFT_TASK_ID)).toBeDefined()
  })

  it('gives the focus card two sittings and the draft card a ticket', () => {
    // These are not decorative: the focus beat's whole claim is "worked in two
    // sittings", and the draft beat's is "maps to a ticket".
    expect(tasks.find((t) => t.id === FOCUS_TASK_ID)!.segments.length).toBe(2)
    expect(tasks.find((t) => t.id === DRAFT_TASK_ID)!.linked_ticket).toBeTruthy()
  })

  it('includes work with no ticket at all', () => {
    // Load-bearing for expectation-setting: it teaches, with a card rather
    // than a sentence, that Meridian does not force everything onto a ticket.
    expect(tasks.some((t) => t.linked_ticket === null)).toBe(true)
  })
})

// The right panel sits beside the example timeline during the whole
// walkthrough, so its numbers are read against the cards. Anything that adds up
// differently reads as the product miscounting the very day it is using to
// prove it counts accurately.
describe('sample overview', () => {
  const tasks = sampleTasks()
  const ov = sampleOverview()

  it('counts Focus as wall-clock time, not the sum of task minutes', () => {
    // This is the one that was actually wrong: two example tasks run
    // concurrently (a talk playing while a test suite ran), so summing task
    // minutes reports 10h 25m for a day whose real footprint is 7h 5m. The
    // marketing demo unions the segments (`unionMinutes`); so must this.
    const summed = tasks.reduce((a, t) => a + t.minutes, 0) * 60
    expect(ov.engagedSeconds).toBe(425 * 60)
    expect(ov.engagedSeconds).toBeLessThan(summed)
  })

  it('keeps app time as its own figures, not derived from the tasks', () => {
    // Apps and tasks are different cuts of a day; these are the demo's own
    // `appMinutes`. Deriving them from the task list would invent a precision
    // neither the demo nor the real app has.
    const byApp = Object.fromEntries(ov.appSessions.map((s) => [s.app, s.dur / 60]))
    expect(byApp).toEqual({
      'Claude Code': 148, 'Google Chrome': 96, GitHub: 74, Slack: 62, 'System Settings': 12,
    })
    // Blank `cat` so these never leak into the category donut.
    for (const s of ov.appSessions) expect(s.cat).toBe('')
  })

  it('keeps category time as a per-task sum covering every card', () => {
    expect(ov.catSessions.length).toBe(tasks.length)
    const total = ov.catSessions.reduce((a, s) => a + s.dur, 0)
    expect(total).toBe(tasks.reduce((a, t) => a + t.minutes, 0) * 60)
    // Blank `app` so these never leak into Time by app.
    for (const s of ov.catSessions) expect(s.app).toBe('')
  })

  it('posts nothing, because the tracker ask has not happened yet', () => {
    // A "synced to Jira" pill on a card would show the end state before the
    // beat that asks the user to connect a tracker, making the ask look moot.
    expect(tasks.every((t) => !t.posted_provider)).toBe(true)
    expect(ov.greetingBody).not.toContain('logged')
    expect(ov.greetingBody).toContain('7h 5m')
  })

  it('counts every ticketed card as a waiting draft', () => {
    expect(ov.pendingCount).toBe(tasks.filter((t) => t.linked_ticket).length)
    expect(ov.pendingCount).toBeGreaterThan(0)
  })

  it('leaves one focus item unfinished', () => {
    // A plan that came out exactly as written is not what a real day looks
    // like, and the timeline has no card for the third item on purpose.
    expect(ov.focusItems.length).toBeGreaterThan(0)
    expect(ov.focusItems.some((t) => !t.is_terminal)).toBe(true)
    expect(ov.focusItems.some((t) => t.is_terminal)).toBe(true)
  })
})

// The walkthrough's two halves are a sequencing property of one async function,
// so nothing type-checks the order. Getting it wrong is silent and only visible
// by sitting through a two-minute run — hence a source-order guard.
describe('walkthrough structure', () => {
  const src = readFileSync(import.meta.dir + '/../components/tutorial/script.ts', 'utf8')
  const at = (needle: string) => {
    const i = src.indexOf(needle)
    if (i < 0) throw new Error(`not found in script.ts: ${needle}`)
    return i
  }

  it('collects the plan and the tracker BEFORE showing the example day', () => {
    // Part one has to run on the user's own empty timeline. If the example is
    // switched on first, the real (blank) day they return to reads as broken —
    // which is the exact failure the example exists to prevent.
    expect(at('data-tour="plan-open"')).toBeLessThan(at('s.showExample(true)'))
    expect(at('s.ask(')).toBeLessThan(at('s.showExample(true)'))
  })

  it('hands the example day back before the closing line', () => {
    // Ending while the example is still on screen would leave invented tasks
    // sitting on the user's timeline after the overlay is gone.
    expect(at('s.showExample(false)')).toBeLessThan(src.lastIndexOf("s.say('')"))
  })

  it('dims the screen for the plan beat and nowhere else', () => {
    // `dim` blurs everything else AND swallows clicks outside the target, so a
    // beat that dims is a beat the user cannot walk away from. That is right
    // exactly once — the daily plan, which every later beat assumes exists.
    // Spreading it to beats the user could reasonably skip turns a walkthrough
    // into a hostage situation.
    const dims = [...src.matchAll(/s\.spotlight\(([^)]*\{[^)]*dim[^)]*\})\)/g)]
    expect(dims.length).toBe(1)
    expect(dims[0][1]).toContain('plan-open')
  })

  it('pairs every dim with a later clear', () => {
    // A dim left fenced around nothing blurs and locks the entire window. The
    // hook clears it whenever the spotlight clears, so the guard is just that
    // the beat does clear its spotlight afterwards.
    expect(at('s.spotlight(null)')).toBeGreaterThan(at('dim: true'))
  })

  it('branches the closing worklog line on the tracker answer', () => {
    // A solo user must not be told Meridian posts to a board they never
    // connected — the product would contradict it within the hour.
    const draft = src.slice(at('── 7. The worklog draft'))
    expect(draft).toContain("usesTracker === 'tracker'")
  })

  it('opens with the title sequence, before any narration', () => {
    // Opening mid-gesture ("click this card") reads as the app seizing control.
    const body = src.slice(at('export async function runScript'))
    const first = body.match(/s\.(?:intro|say|next)\(/)
    expect(first?.[0]).toBe('s.intro(')
  })

  it('says both what this is and how long it takes', () => {
    // Two facts, and the duration is the load-bearing one: without it a tour
    // that takes over the whole window is an indefinite takeover.
    const intro = INTRO_LINES.join(' ')
    expect(intro).toMatch(/take you around/i)
    expect(intro).toMatch(/2 minutes/i)
  })

  it('keeps the title card to two short lines', () => {
    // Each word is revealed on its own step, so length is duration: the open is
    // a title card, and a third line (or a long one) turns it into a slideshow
    // in front of someone who has not seen the product yet.
    expect(INTRO_LINES.length).toBe(2)
    for (const l of INTRO_LINES) expect(l.split(/\s+/).length).toBeLessThanOrEqual(10)
  })

  it('goes from the title card straight to the plan, with nothing in between', () => {
    // No explain-the-product beat between them. A tour that opens with a
    // paragraph about what Meridian is spends the user's attention before
    // asking for anything, and every sentence in it means more once their own
    // tasks are on screen. What the product does gets SHOWN, later, on their
    // own plan.
    const between = src.slice(at('await s.intro('), at('dim: true'))
    expect(between).not.toMatch(/s\.next\(/)
  })

  it('uses plain hyphens in the intro, per the user-facing text rule', () => {
    for (const l of INTRO_LINES) expect(l).not.toMatch(/[—–]|--/)
  })

  it('advances narration on Next, not on a timer', () => {
    // A voiceover plays over these lines, so hardcoded millisecond guesses
    // cannot be synced to it — and a reader slower than the guess loses the
    // sentence. `pause` is allowed only for mechanical gaps (a modal's open
    // transition), which are all well under a second and a half. The title
    // sequence is the one exception, and it is timed inside TutorialIntro
    // precisely because it has nothing on screen for a reader to lose.
    const body = src.slice(at('export async function runScript'))
    expect(body).toContain('s.next(')
    for (const m of body.matchAll(/s\.pause\((\d+)\)/g)) {
      expect(Number(m[1])).toBeLessThanOrEqual(1500)
    }
  })

  // Most first runs have no AI provider connected, so pressing "Draft with AI"
  // produces a connect card instead of a draft. These pin the detour that
  // handles it — every one of them is invisible to a type checker.
  describe('the no-provider detour off the draft beat', () => {
    const detour = () => src.slice(at('data-tour="ai-connect"'), at('2c. The draft landing'))

    it('branches on what actually appeared, not on an assumption', () => {
      // Without the branch the tour narrates "a title and a description, from
      // one line" over a screen showing neither - the most confusing thing a
      // walkthrough can do.
      expect(src).toMatch(/await s\.appeared\('\[data-tour="ai-connect"\]'/)
      expect(at('await s.appeared')).toBeGreaterThan(at('data-tour="task-draft"'))
      expect(at('await s.appeared')).toBeLessThan(at('2c. The draft landing'))
    })

    it('lets the card arrive and be read before pointing at it', () => {
      // The composer holds a "Checking your AI…" beat and the card fades in over
      // ~600ms. A spotlight landing during that is a second surprise on top of
      // the first. The pause must come BEFORE the spotlight, not after.
      const d = detour()
      expect(d.indexOf('s.pause(')).toBeLessThan(d.indexOf('s.spotlight('))
      const first = Number(d.match(/s\.pause\((\d+)\)/)?.[1])
      expect(first).toBeGreaterThanOrEqual(1000)
    })

    it('waits for the planner to come BACK, not for a modal to be closed', () => {
      // The app returns itself: once the provider is written, SettingsModal fires
      // `onLockSatisfied`, the shell arms a resume and reopens the planner, and the
      // draft the user already asked for runs on its own. Waiting on a close button
      // would race that - and that button is DISABLED until the step is done, so the
      // tour would have been waiting on a control that cannot be pressed.
      const d = detour()
      expect(d).toContain('data-tour="task-note"')
      expect(d).not.toContain("s.openModal('plan')")
      expect(d).not.toContain('modal-close')
    })

    it('rejoins the normal beats instead of re-asking for the draft click', () => {
      // The draft is already running by the time the planner is back. Asking for a
      // click that has effectively happened leaves the tour waiting on a button whose
      // work is done, with the fields filling in behind the instruction to press it.
      const d = detour()
      expect(d).not.toMatch(/waitForClick\('\[data-tour="task-draft"\]'/)
      // …and the beat it falls into is the one that watches the title fill.
      expect(at('2c. The draft landing')).toBeLessThan(at('waitForValue(\'[data-tour="task-title"]\''))
    })

    it('never dims - the detour is skippable, unlike the plan beat', () => {
      expect(detour()).not.toContain('dim')
    })

    it('waits on the subscription gate without taking a side', () => {
      // Settings opens LOCKED here, which puts the picker on its subscription question
      // rather than the provider grid. The tour must wait on EITHER answer and must not
      // ring one of them: the recommendation is the badge's job, and a spotlight on one
      // card makes the other read as the wrong answer.
      const d = detour()
      expect(d).toContain('gate-subscription')
      expect(d).toContain('gate-free')
      expect(d).not.toMatch(/spotlight\([^)]*gate-/)
    })

    it('does not walk them through a provider picker twice', () => {
      // Beat 6 asks for a provider too. Sending someone back there four beats after they
      // connected reads as the tour having lost track - and as their connection not
      // counting.
      expect(src).toContain('connectedAi = true')
      expect(src).toContain('if (connectedAi) {')
      expect(at('if (connectedAi) {')).toBeGreaterThan(at('── 6. AI ask'))
    })
  })
})

describe('fmtSetupElapsed', () => {
  it('reads as seconds under a minute', () => {
    expect(fmtSetupElapsed(9)).toBe('9s')
    expect(fmtSetupElapsed(59)).toBe('59s')
  })

  it('drops a zero seconds remainder', () => {
    expect(fmtSetupElapsed(120)).toBe('2m')
  })

  it('keeps a non-zero remainder', () => {
    expect(fmtSetupElapsed(134)).toBe('2m 14s')
  })
})
