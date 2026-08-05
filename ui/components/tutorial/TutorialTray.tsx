//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// A stand-in for the menu-bar popover, for the one beat that teaches the off
// switch.
//
// WHY A DRAWING RATHER THAN THE REAL THING. The tray popover is a separate
// Tauri window hanging off the macOS menu bar, outside this window entirely -
// the walkthrough cannot open it, point at it, or know when the user has
// clicked something in it. So the choice is between teaching the control with a
// replica and not teaching it at all, and "how do I turn this off" is the
// question an ambient recorder must answer before it is trusted, not after.
//
// AND NOTHING IT DOES IS REAL. Pressing Pause here pauses nothing: capture is a
// daemon-level setting, and a walkthrough that silently stops recording on its
// way past would leave a new user with a product that quietly does not work,
// having been told the opposite two beats later. The beat says so out loud (see
// `scriptDay.ts`) rather than letting the countdown imply otherwise.
//
// The markup mirrors `tray/src/index.html`'s status card - the same wording,
// the same four durations, in the same order - so the real popover is
// recognised the first time it is opened for real.
//
// # Who calls this
// [`TutorialScreen`], while `Stage.demoTray(true)` is in effect.
//
// # Related
// - `./scriptDay.ts` — the beats that drive it
// - `tray/src/index.html` — the popover this imitates

import { useEffect, useState } from 'react'
import { MeridianMark } from '@/components/timeline/Toolbar'

/** Where the OS actually puts the tray icon - and this beat is ABOUT where it
 *  is, so getting it wrong is not a cosmetic slip. macOS hangs it off the menu
 *  bar along the top; Windows puts it in the notification area at the bottom
 *  right, next to the clock. A Windows user told to look up at their menu bar is
 *  being sent to a place their machine does not have.
 *
 *  Read from the user agent rather than a Tauri call: this renders in the plain
 *  browser during `?tour=day` too, and the answer is only ever used to decide
 *  which corner to draw in and which word to call it. `ua` is injectable for the
 *  unit test; everything else passes nothing. Defaults to macOS when there is no
 *  `navigator` at all (the static export's prerender). */
export function trayIsAtTheTop(ua?: string): boolean {
  const agent = ua ?? (typeof navigator === 'undefined' ? '' : navigator.userAgent)
  return !/Windows|Win32|Win64/i.test(agent)
}

const OPTIONS: { id: string; label: string; hint: string }[] = [
  { id: 'tray-15m', label: 'Pause for 15 minutes', hint: 'back at 3:15 PM' },
  { id: 'tray-1h', label: 'Pause for 1 hour', hint: 'back at 4:00 PM' },
  { id: 'tray-tomorrow', label: 'Pause until tomorrow', hint: 'back at 9:00 AM' },
  { id: 'tray-indefinite', label: 'Pause indefinitely', hint: 'until I resume' },
]

export function TutorialTray() {
  const [open, setOpen] = useState(false)
  // Same two-step as the real popover: "Pause capture" reveals the durations
  // rather than pausing on the spot, so nobody stops their own recording by
  // reaching for the button that says what it does.
  const [showOpts, setShowOpts] = useState(false)
  const [paused, setPaused] = useState<string | null>(null)
  // Resolved after mount - `navigator` does not exist during the static export's
  // prerender. macOS is the default because it is what the tour is drawn for and
  // what most first runs are on; a Windows user gets the correct corner on the
  // first paint after hydration, well before this beat can run.
  const [top, setTop] = useState(true)
  useEffect(() => { setTop(trayIsAtTheTop(navigator.userAgent)) }, [])

  return (
    // Pinned to the corner the OS actually keeps the tray in - top-right under
    // the macOS menu bar, bottom-right in the Windows notification area. Above
    // the summary (z-5) and below the tour overlay (z-9000).
    //
    // PARKED BELOW THE SKIP PILLS on macOS, not at y=0. The overlay's own escape
    // hatches live at the top-right at z-9000, so a strip at `top-0 right-0` sat
    // exactly underneath them - and the beat asks the user to click the tray
    // icon, which was swallowed by "Skip tour" every time. Cleared rather than
    // reordered: Skip must stay clickable throughout, and the strip is a drawing
    // whose meaning comes from its label, not its pixel position. The bottom
    // corner has no such collision.
    <div className={`absolute right-0 p-3 flex ${top ? 'flex-col' : 'flex-col-reverse'} items-end`}
      style={{ zIndex: 20, ...(top ? { top: 44 } : { bottom: 0 }) }}>
      {/* The strip itself. Labelled, because a Meridian mark floating in the
          corner of the app window is not self-evidently a drawing OF the menu
          bar or the notification area - and the whole point of the beat is where
          the control lives. */}
      <div className="flex items-center gap-2 rounded-lg px-2.5 py-1.5"
        style={{ background: 'rgba(20,16,40,0.86)', border: '1px solid rgba(255,255,255,.14)' }}>
        <span style={{ fontSize: 10.5, color: 'rgba(255,255,255,.6)' }}>
          {top ? 'your menu bar' : 'your system tray'}
        </span>
        <button data-tour="tray-icon" onClick={() => setOpen(true)}
          aria-label={top ? 'Meridian in the menu bar' : 'Meridian in the system tray'}
          className="inline-flex items-center justify-center rounded-md"
          style={{
            width: 24, height: 24, cursor: 'pointer', border: 'none',
            background: open ? 'rgba(255,255,255,.16)' : 'transparent',
          }}>
          <MeridianMark size={15} />
        </button>
      </div>

      {open && (
        <div data-tour="tray-popover"
          className={`${top ? 'mt-2' : 'mb-2'} rounded-xl overflow-hidden mer-pop`}
          style={{
            width: 268, background: 'var(--t-panel)',
            border: '1px solid var(--t-card-border)', boxShadow: 'var(--mt-modal-shadow)',
          }}>
          <div className="flex items-center gap-2 px-3.5 py-3 border-b" style={{ borderColor: 'var(--t-hair)' }}>
            <MeridianMark size={16} />
            <span style={{ font: '700 13px var(--font-sans)', color: 'var(--t-title)' }}>Meridian</span>
          </div>

          <div className="p-3">
            <div className="flex items-center gap-2">
              <span className="rounded-full shrink-0" style={{
                width: 8, height: 8,
                background: paused ? 'var(--color-state-pending)' : 'var(--color-state-approved)',
              }} />
              <div className="min-w-0">
                <p className="mt-body-sm" style={{ color: 'var(--t-title)', fontWeight: 700 }}>
                  {paused ? 'Paused' : 'Capturing'}
                </p>
                <p style={{ fontSize: 11, color: 'var(--t-faint)' }}>
                  {paused ?? 'Meridian is watching this hour'}
                </p>
              </div>
            </div>

            {paused ? (
              <div className="flex items-center justify-between mt-3 rounded-lg px-3 py-2"
                style={{ background: 'var(--t-box)' }}>
                <span style={{ font: '700 15px var(--font-sans)', color: 'var(--t-title)' }}>15:00</span>
                <span className="mt-body-sm" style={{ color: 'var(--color-state-proposal)', fontWeight: 700 }}>
                  Resume now
                </span>
              </div>
            ) : (
              <>
                <button data-tour="tray-pause" onClick={() => setShowOpts(true)}
                  className="w-full rounded-lg mt-3 hover:opacity-90"
                  style={{
                    padding: '9px 0', fontSize: 12.5, fontWeight: 700, color: '#fff',
                    background: 'var(--color-state-proposal)', border: 'none', cursor: 'pointer',
                  }}>
                  Pause capture
                </button>
                {showOpts && <div data-tour="tray-options" className="flex flex-col gap-1 mt-2">
                  {OPTIONS.map(o => (
                    <button key={o.id} data-tour={o.id} onClick={() => setPaused(o.hint)}
                      className="flex items-center justify-between rounded-lg px-2.5 py-2 text-left hover:opacity-80"
                      style={{ background: 'var(--t-box)', border: '1px solid var(--t-card-border)', cursor: 'pointer' }}>
                      <span className="mt-body-sm" style={{ color: 'var(--t-title)' }}>{o.label}</span>
                      <span style={{ fontSize: 10.5, color: 'var(--t-faint)' }}>{o.hint}</span>
                    </button>
                  ))}
                </div>}
              </>
            )}
          </div>
        </div>
      )}
    </div>
  )
}
