//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The walkthrough's narration, typed rather than printed.
//
// WHY IT IS TYPED AT ALL. A line that appears whole is read in a second and
// then sits there, and the tour's pacing collapses into "how long until the
// button appears". Arriving a character at a time, the line is read at roughly
// the speed it is written - which is also roughly the speed it is spoken, so a
// voiceover and the screen stay together without either being synced to the
// other. `TutorialIntro` reached the same conclusion a word at a time; this is
// that idea, generalised to every surface that speaks.
//
// ONE IMPLEMENTATION, THREE CONSUMERS - the takeover hero, the anchored
// caption, and the opening. Three copies would mean three pacing constants, and
// the two nobody is currently looking at drift silently.
//
// # Who calls this
// [`TutorialTakeover`] for its hero line; [`TutorialOverlay`] for the caption.
//
// # Related
// - `./engine.ts` — `Stage.takeover`, the beat whose copy this reveals

import { useCallback, useEffect, useRef, useState } from 'react'

/** Milliseconds between characters.
 *
 *  Fast enough that a 90-character line lands in about two seconds, slow enough
 *  to read as writing rather than as a render. Deliberately quicker than the
 *  45ms `useTutorial` types into a FORM field with: that one is imitating a
 *  person at a keyboard and should look like effort, this one is a voice and
 *  should not make the reader wait on it. */
const CHAR_MS = 22

/** Reveal `text` one character at a time.
 *
 *  Returns the visible slice, whether it has finished, and a `finish` that
 *  lands the rest instantly. That escape hatch is not a nicety: a line typed at
 *  reading speed is a PACE, not a gate, and someone re-running the tour or
 *  simply reading faster has to be able to take the whole line at once.
 *  Otherwise the animation that exists to make narration readable becomes the
 *  thing making it unreadable.
 *
 *  Restarts whenever `text` changes, which is what makes it usable for a
 *  caption that is reassigned every beat rather than remounted.
 *
 *  HONOURS `prefers-reduced-motion`, in which case the text is simply there.
 *  Typing is motion in the literal sense - the reason someone turns that
 *  setting on is that moving text is hard for them to read, and a tour is the
 *  worst place to ignore it. */
export function useTypewriter(text: string): {
  /** What to render right now. */
  shown: string
  /** True once every character is out - callers gate their CTA on this. */
  done: boolean
  /** Land the remaining characters immediately. Safe to call when done. */
  finish: () => void
} {
  const [shown, setShown] = useState(text)
  const [done, setDone] = useState(true)
  // Held so `finish` can land the full string without depending on the render
  // that started the run - it is called from an event handler, which may fire
  // between renders.
  const fullRef = useRef(text)
  const timerRef = useRef<ReturnType<typeof setInterval> | null>(null)

  const stop = useCallback(() => {
    if (timerRef.current !== null) {
      clearInterval(timerRef.current)
      timerRef.current = null
    }
  }, [])

  const finish = useCallback(() => {
    stop()
    setShown(fullRef.current)
    setDone(true)
  }, [stop])

  useEffect(() => {
    fullRef.current = text
    const reduced = typeof window !== 'undefined'
      && window.matchMedia?.('(prefers-reduced-motion: reduce)').matches
    if (!text || reduced) {
      setShown(text)
      setDone(true)
      return
    }
    setShown('')
    setDone(false)
    let i = 0
    const id = setInterval(() => {
      i += 1
      if (i >= text.length) {
        clearInterval(id)
        timerRef.current = null
        setShown(text)
        setDone(true)
        return
      }
      setShown(text.slice(0, i))
    }, CHAR_MS)
    timerRef.current = id
    // Clears on unmount AND on every text change. Without it the interval
    // outlives the component and keeps calling setState on something React has
    // unmounted - which, in a walkthrough the user has just skipped, is one
    // leaked timer per beat.
    return () => { clearInterval(id); timerRef.current = null }
  }, [text])

  return { shown, done, finish }
}
