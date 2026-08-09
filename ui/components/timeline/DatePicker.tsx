//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// A small interactive calendar popover for picking a due date — replaces the
// native `<input type="date">` (CleanupCard/HygieneDialog's `Control`), whose
// OS-drawn picker looks and behaves differently per platform and reads as an
// afterthought next to the rest of this app's controls. Same open/close
// contract as StatusPicker (outside-click + Escape), and the same `value`/
// `onChange` shape the native input had (`YYYY-MM-DD` string, `''` = unset),
// so it drops straight into `Control` with no change to the fix-apply flow.
//
// Rendered through a portal into `document.body`, positioned by the trigger's
// live `getBoundingClientRect()` (recomputed on scroll/resize while open),
// rather than absolutely inside the trigger's own DOM position. CleanupCard's
// outer card is `overflow-hidden` (for its rounded corners + accent border),
// so an in-place absolute popover got silently clipped by that ancestor —
// reading as the popup "cutting" against the card edge before the card's own
// layout settled. A portal escapes that ancestor entirely, so it's clipped
// only by the real viewport, same as any native OS-level dropdown.

'use client'

import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { createPortal } from 'react-dom'

const WEEKDAY_LABELS = ['Su', 'Mo', 'Tu', 'We', 'Th', 'Fr', 'Sa']
const MONTH_LABELS = [
  'January', 'February', 'March', 'April', 'May', 'June',
  'July', 'August', 'September', 'October', 'November', 'December',
]

function pad2(n: number): string {
  return n < 10 ? `0${n}` : String(n)
}

/** Local YYYY-MM-DD (never UTC — a due date is a calendar day, not an instant). */
function toDateKey(d: Date): string {
  return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}`
}

/** Parse a `YYYY-MM-DD` string as a local date, or `null` if empty/malformed. */
function parseDateKey(value: string): Date | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value)
  if (!m) return null
  const d = new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3]))
  return Number.isNaN(d.getTime()) ? null : d
}

function formatDisplay(value: string): string {
  const d = parseDateKey(value)
  if (!d) return ''
  return `${MONTH_LABELS[d.getMonth()].slice(0, 3)} ${d.getDate()}, ${d.getFullYear()}`
}

export function DatePicker({ value, onChange, placeholder = 'Set due date' }: {
  value: string
  onChange: (v: string) => void
  placeholder?: string
}) {
  const [open, setOpen] = useState(false)
  const selected = useMemo(() => parseDateKey(value), [value])
  const today = useMemo(() => new Date(), [])
  // The month currently shown — seeded from the selected date (or today), then
  // owned locally so ‹/› can browse without touching the real value until a day
  // is actually clicked.
  const [viewYear, setViewYear] = useState(() => (selected ?? today).getFullYear())
  const [viewMonth, setViewMonth] = useState(() => (selected ?? today).getMonth())
  const triggerRef = useRef<HTMLButtonElement>(null)
  const popoverRef = useRef<HTMLDivElement>(null)
  // Popover's fixed-position coords, in viewport pixels — null until the first
  // measurement lands, so nothing renders (and nothing can flash unpositioned
  // at the origin) before that.
  const [coords, setCoords] = useState<{ top: number; left: number } | null>(null)

  // Re-measure the trigger and place the popover just under it, clamped so a
  // trigger near the right/bottom edge doesn't push the 260px-wide, ~300px-tall
  // panel off-screen. Runs on open, and on every scroll/resize while open so a
  // reflow elsewhere (e.g. the modal's own content settling) can't leave it
  // pointing at stale coordinates.
  const reposition = () => {
    const el = triggerRef.current
    if (!el) return
    const r = el.getBoundingClientRect()
    const width = 260
    const left = Math.min(r.left, window.innerWidth - width - 8)
    const top = Math.min(r.bottom + 4, window.innerHeight - 320)
    setCoords({ top: Math.max(8, top), left: Math.max(8, left) })
  }

  useLayoutEffect(() => {
    if (!open) return
    reposition()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open])

  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node
      if (triggerRef.current?.contains(t) || popoverRef.current?.contains(t)) return
      setOpen(false)
    }
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') setOpen(false) }
    document.addEventListener('mousedown', onDown)
    document.addEventListener('keydown', onKey)
    // capture:true — a scroll deep inside the modal's own scroll region doesn't
    // bubble to window in every browser, but it does fire in the capture phase.
    window.addEventListener('scroll', reposition, true)
    window.addEventListener('resize', reposition)
    return () => {
      document.removeEventListener('mousedown', onDown)
      document.removeEventListener('keydown', onKey)
      window.removeEventListener('scroll', reposition, true)
      window.removeEventListener('resize', reposition)
    }
  }, [open])

  const toggle = () => {
    const next = !open
    setOpen(next)
    if (next) {
      // Re-seed the visible month on every open, so re-opening after a stale
      // browse (e.g. left on a far-away month last time) starts back at the
      // selected/current month rather than wherever ‹/› last left it.
      const base = selected ?? today
      setViewYear(base.getFullYear())
      setViewMonth(base.getMonth())
    }
  }

  const pick = (d: Date) => { onChange(toDateKey(d)); setOpen(false) }

  const shiftMonth = (delta: number) => {
    let y = viewYear, m = viewMonth + delta
    if (m < 0) { m = 11; y -= 1 } else if (m > 11) { m = 0; y += 1 }
    setViewYear(y); setViewMonth(m)
  }

  // Six full weeks (42 cells), Sunday-first — covers every month's layout
  // without the grid ever growing/shrinking a row as you browse.
  const cells = useMemo(() => {
    const firstOfMonth = new Date(viewYear, viewMonth, 1)
    const gridStart = new Date(viewYear, viewMonth, 1 - firstOfMonth.getDay())
    return Array.from({ length: 42 }, (_, i) => {
      const d = new Date(gridStart.getFullYear(), gridStart.getMonth(), gridStart.getDate() + i)
      return d
    })
  }, [viewYear, viewMonth])

  const todayKey = toDateKey(today)

  return (
    <>
      <button
        ref={triggerRef}
        onClick={toggle}
        className="flex-1 min-w-0 text-left mt-body-sm inline-flex items-center gap-1.5 rounded-md px-2.5 py-1.5"
        style={{ border: '1px solid var(--t-input-border)', background: 'var(--t-input)', color: value ? 'var(--t-title)' : 'var(--t-faint)' }}>
        <span aria-hidden style={{ fontSize: 12 }}>📅</span>
        <span className="flex-1 truncate">{value ? formatDisplay(value) : placeholder}</span>
        <span style={{ fontSize: 9, opacity: 0.6 }}>▾</span>
      </button>

      {open && coords && typeof document !== 'undefined' && createPortal(
        <div ref={popoverRef} className="fixed z-50 rounded-xl overflow-hidden"
          style={{ top: coords.top, left: coords.left, width: 260, background: 'var(--t-card)', border: '1px solid var(--t-card-border)', boxShadow: '0 12px 34px -12px rgba(0,0,0,0.4)' }}>
          <div className="flex items-center justify-between px-3 py-2.5" style={{ borderBottom: '1px solid var(--t-hair)' }}>
            <button onClick={() => shiftMonth(-1)} aria-label="Previous month"
              className="inline-flex items-center justify-center rounded-md" style={{ width: 22, height: 22, color: 'var(--t-muted)' }}>‹</button>
            <p className="mt-label" style={{ color: 'var(--t-title)' }}>{MONTH_LABELS[viewMonth]} {viewYear}</p>
            <button onClick={() => shiftMonth(1)} aria-label="Next month"
              className="inline-flex items-center justify-center rounded-md" style={{ width: 22, height: 22, color: 'var(--t-muted)' }}>›</button>
          </div>

          <div className="px-2.5 pt-2.5">
            <div className="grid grid-cols-7">
              {WEEKDAY_LABELS.map(w => (
                <span key={w} className="mt-mono-sm text-center" style={{ fontSize: 9.5, color: 'var(--t-faint)', padding: '2px 0' }}>{w}</span>
              ))}
            </div>
            <div className="grid grid-cols-7">
              {cells.map(d => {
                const key = toDateKey(d)
                const inMonth = d.getMonth() === viewMonth
                const isSelected = key === value
                const isToday = key === todayKey
                return (
                  <button key={key} onClick={() => pick(d)}
                    className="mt-mono-sm inline-flex items-center justify-center rounded-md m-0.5"
                    style={{
                      height: 28,
                      fontSize: 11.5,
                      color: isSelected ? '#fff' : inMonth ? 'var(--t-title)' : 'var(--t-faint-2)',
                      background: isSelected ? 'var(--btn-primary-bg)' : 'transparent',
                      border: isToday && !isSelected ? '1px solid var(--t-accent)' : '1px solid transparent',
                      fontWeight: isSelected || isToday ? 700 : 400,
                    }}>
                    {d.getDate()}
                  </button>
                )
              })}
            </div>
          </div>

          <div className="flex items-center justify-between px-3 py-2.5" style={{ borderTop: '1px solid var(--t-hair)' }}>
            <button onClick={() => pick(today)} className="mt-body-sm" style={{ color: 'var(--t-accent)', fontWeight: 700 }}>Today</button>
            {value && (
              <button onClick={() => { onChange(''); setOpen(false) }} className="mt-body-sm" style={{ color: 'var(--t-faint)' }}>Clear</button>
            )}
          </div>
        </div>,
        document.body,
      )}
    </>
  )
}
