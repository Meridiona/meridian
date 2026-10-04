//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The toolbar's "jump to a day" calendar. The date label between the ‹ › arrows
// is the trigger; clicking it opens a month grid anchored under the label.
// Picking a day calls `onPick(YYYY-MM-DD)` and closes. Days after today are
// disabled (the dashboard never navigates past today - see `shiftDay`).
//
// - Title click flips the panel to a 12-month grid with year arrows, so a day
//   months back is three clicks, not thirty.
// - Keyboard: arrows move by day/week, PageUp/PageDown by month, Home/End to the
//   start/end of the week, Enter picks, Esc closes. Roving tabindex, so Tab
//   leaves the grid in one stop.
// - Rendered through a portal with fixed positioning (same reason as
//   `DatePicker`): no ancestor's `overflow` can clip it.
//
// Dates are LOCAL calendar days as `YYYY-MM-DD` strings, compared lexically -
// never via UTC, and never via Date arithmetic across DST (noon-anchored).
//
// # Who calls this
// `Toolbar.tsx`, in place of the static date label.

'use client'

import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { createPortal } from 'react-dom'

const WEEKDAYS = ['Su', 'Mo', 'Tu', 'We', 'Th', 'Fr', 'Sa']
const MONTHS = [
  'January', 'February', 'March', 'April', 'May', 'June',
  'July', 'August', 'September', 'October', 'November', 'December',
]
const PANEL_W = 300

const pad2 = (n: number) => (n < 10 ? `0${n}` : String(n))

/** Local `YYYY-MM-DD` for a Date. */
export function keyOf(d: Date): string {
  return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}`
}

/** Parse `YYYY-MM-DD` as a local, noon-anchored Date (DST-safe). */
export function parseKey(key: string): Date {
  const [y, m, d] = key.split('-').map(Number)
  return new Date(y, m - 1, d, 12)
}

/** `key` moved by `days`, as a key. */
export function addDays(key: string, days: number): string {
  const d = parseKey(key)
  d.setDate(d.getDate() + days)
  return keyOf(d)
}

/** `key` moved by `months`, clamped to the target month's last day. */
export function addMonths(key: string, months: number): string {
  const d = parseKey(key)
  const day = d.getDate()
  d.setDate(1)
  d.setMonth(d.getMonth() + months)
  const last = new Date(d.getFullYear(), d.getMonth() + 1, 0).getDate()
  d.setDate(Math.min(day, last))
  return keyOf(d)
}

/** Six Sunday-first weeks (42 keys) so the grid never changes height as you browse. */
export function monthGrid(year: number, month: number): string[] {
  const first = new Date(year, month, 1, 12)
  const start = new Date(year, month, 1 - first.getDay(), 12)
  return Array.from({ length: 42 }, (_, i) => {
    const d = new Date(start)
    d.setDate(start.getDate() + i)
    return keyOf(d)
  })
}

const CalendarGlyph = () => (
  <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden="true">
    <rect x="2" y="3" width="12" height="11" rx="2.5" stroke="currentColor" strokeWidth="1.4" />
    <path d="M2 6.6h12M5.4 1.8v2.4M10.6 1.8v2.4" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
  </svg>
)

const Chevron = ({ dir }: { dir: 'left' | 'right' }) => (
  <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden="true">
    <path d={dir === 'left' ? 'M10 3.5 5.5 8l4.5 4.5' : 'M6 3.5 10.5 8 6 12.5'}
      stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" />
  </svg>
)

function ArrowBtn({ dir, label, onClick, disabled }: {
  dir: 'left' | 'right'; label: string; onClick: () => void; disabled?: boolean
}) {
  return (
    <button type="button" onClick={onClick} disabled={disabled} aria-label={label}
      className="mt-cal-arrow inline-flex items-center justify-center rounded-lg"
      style={{ width: 28, height: 28, color: disabled ? 'var(--t-faint-2)' : 'var(--t-muted)' }}>
      <Chevron dir={dir} />
    </button>
  )
}

export function DayCalendar({ day, isToday, label, onPick }: {
  /** The day being viewed, `YYYY-MM-DD`. */
  day: string
  isToday: boolean
  /** Text shown on the trigger ("Today" / "Tue, Jun 30"). */
  label: string
  onPick: (day: string) => void
}) {
  const [open, setOpen] = useState(false)
  const [mode, setMode] = useState<'days' | 'months'>('days')
  // The month on screen (the 1st of it), independent of the picked day until a pick.
  const [view, setView] = useState(() => day.slice(0, 7) + '-01')
  const [focusKey, setFocusKey] = useState(day)
  const [coords, setCoords] = useState<{ top: number; left: number } | null>(null)
  const triggerRef = useRef<HTMLButtonElement>(null)
  const panelRef = useRef<HTMLDivElement>(null)

  // Recomputed each open so a tray left running past midnight still caps at the real today.
  const todayKey = useMemo(() => keyOf(new Date()), [open])
  const yesterdayKey = addDays(todayKey, -1)
  const viewDate = parseKey(view)
  const viewYear = viewDate.getFullYear()
  const viewMonth = viewDate.getMonth()
  const cells = useMemo(() => monthGrid(viewYear, viewMonth), [viewYear, viewMonth])
  const nextMonthDisabled = addMonths(view, 1) > todayKey

  const reposition = () => {
    const el = triggerRef.current
    if (!el) return
    const r = el.getBoundingClientRect()
    const left = Math.min(r.left + r.width / 2 - PANEL_W / 2, window.innerWidth - PANEL_W - 12)
    setCoords({ top: r.bottom + 10, left: Math.max(12, left) })
  }

  useLayoutEffect(() => { if (open) reposition() }, [open])

  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node
      if (triggerRef.current?.contains(t) || panelRef.current?.contains(t)) return
      setOpen(false)
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') { setOpen(false); triggerRef.current?.focus() }
    }
    document.addEventListener('mousedown', onDown)
    document.addEventListener('keydown', onKey)
    window.addEventListener('resize', reposition)
    return () => {
      document.removeEventListener('mousedown', onDown)
      document.removeEventListener('keydown', onKey)
      window.removeEventListener('resize', reposition)
    }
  }, [open])

  // Move DOM focus to the roving day after it changes (and on open).
  useEffect(() => {
    if (!open || mode !== 'days') return
    panelRef.current?.querySelector<HTMLButtonElement>(`[data-day="${focusKey}"]`)?.focus()
  }, [open, mode, focusKey, view])

  const toggle = () => {
    if (!open) {
      setMode('days')
      setView(day.slice(0, 7) + '-01')
      setFocusKey(day)
    }
    setOpen(o => !o)
  }

  const pick = (key: string) => {
    if (key > todayKey) return
    setOpen(false)
    triggerRef.current?.focus()
    if (key !== day) onPick(key)
  }

  const focusDay = (key: string) => {
    const capped = key > todayKey ? todayKey : key
    setFocusKey(capped)
    setView(capped.slice(0, 7) + '-01')
  }

  const onGridKey = (e: React.KeyboardEvent) => {
    const step: Record<string, () => string> = {
      ArrowLeft: () => addDays(focusKey, -1),
      ArrowRight: () => addDays(focusKey, 1),
      ArrowUp: () => addDays(focusKey, -7),
      ArrowDown: () => addDays(focusKey, 7),
      PageUp: () => addMonths(focusKey, -1),
      PageDown: () => addMonths(focusKey, 1),
      Home: () => addDays(focusKey, -parseKey(focusKey).getDay()),
      End: () => addDays(focusKey, 6 - parseKey(focusKey).getDay()),
    }
    const next = step[e.key]
    if (!next) return
    e.preventDefault()
    focusDay(next())
  }

  return (
    <>
      <button ref={triggerRef} type="button" onClick={toggle}
        aria-haspopup="dialog" aria-expanded={open} aria-label={`Pick a day, viewing ${label}`}
        className="mt-cal-trigger inline-flex items-center justify-center gap-2 rounded-lg px-2.5 py-1.5 min-w-32 whitespace-nowrap"
        style={{ color: 'var(--t-title)' }}>
        <span style={{ color: 'var(--t-accent)', display: 'inline-flex' }}><CalendarGlyph /></span>
        <span className="mt-toolbar-date">{label}</span>
      </button>

      {open && coords && typeof document !== 'undefined' && createPortal(
        <div ref={panelRef} role="dialog" aria-label="Pick a day"
          className="mt-cal-pop fixed z-50 rounded-2xl"
          style={{
            top: coords.top, left: coords.left, width: PANEL_W,
            background: 'var(--t-card)', border: '1px solid var(--t-card-border)',
            boxShadow: '0 24px 60px -18px rgba(33,29,61,0.38), 0 4px 14px -6px rgba(33,29,61,0.16)',
          }}>
          <div className="flex items-center justify-between px-3 pt-3 pb-2">
            <ArrowBtn dir="left" label={mode === 'days' ? 'Previous month' : 'Previous year'}
              onClick={() => setView(addMonths(view, mode === 'days' ? -1 : -12))} />
            <button type="button" onClick={() => setMode(m => (m === 'days' ? 'months' : 'days'))}
              aria-label={mode === 'days' ? 'Choose month and year' : 'Back to days'}
              className="mt-cal-arrow inline-flex items-baseline gap-1.5 rounded-lg px-2.5 py-1"
              style={{ color: 'var(--t-title)' }}>
              <span style={{ font: '700 15px var(--font-sans)' }}>
                {mode === 'days' ? MONTHS[viewMonth] : viewYear}
              </span>
              {mode === 'days' && <span style={{ font: '500 15px var(--font-sans)', color: 'var(--t-faint)' }}>{viewYear}</span>}
            </button>
            <ArrowBtn dir="right" label={mode === 'days' ? 'Next month' : 'Next year'}
              onClick={() => setView(addMonths(view, mode === 'days' ? 1 : 12))}
              disabled={mode === 'days' ? nextMonthDisabled : viewYear >= parseKey(todayKey).getFullYear()} />
          </div>

          {mode === 'days' ? (
            <div className="px-3 pb-2" onKeyDown={onGridKey}>
              <div className="grid grid-cols-7 pb-1">
                {WEEKDAYS.map(w => (
                  <span key={w} className="mt-mono-sm text-center"
                    style={{ fontSize: 10.5, fontWeight: 600, letterSpacing: '0.04em', color: 'var(--t-faint)', padding: '4px 0' }}>{w}</span>
                ))}
              </div>
              <div className="grid grid-cols-7" role="grid">
                {cells.map(key => {
                  const d = parseKey(key)
                  const inMonth = d.getMonth() === viewMonth
                  const future = key > todayKey
                  const selected = key === day
                  const isTodayCell = key === todayKey
                  return (
                    <button key={key} type="button" data-day={key} disabled={future}
                      tabIndex={key === focusKey ? 0 : -1}
                      aria-label={d.toLocaleDateString('en-US', { weekday: 'long', month: 'long', day: 'numeric', year: 'numeric' })}
                      aria-current={isTodayCell ? 'date' : undefined}
                      aria-pressed={selected}
                      onClick={() => pick(key)}
                      className="mt-cal-day mt-mono-sm inline-flex items-center justify-center"
                      style={{
                        height: 36, margin: 1, borderRadius: 10, fontSize: 13,
                        fontWeight: selected || isTodayCell ? 700 : 500,
                        color: selected ? '#fff'
                          : future ? 'var(--t-faint-2)'
                          : inMonth ? 'var(--t-title)' : 'var(--t-faint)',
                        opacity: future ? 0.45 : inMonth ? 1 : 0.7,
                        background: selected ? 'var(--btn-primary-bg)' : 'transparent',
                        boxShadow: selected ? '0 6px 14px -6px var(--btn-primary-bg)'
                          : isTodayCell ? 'inset 0 0 0 1.5px var(--t-accent)' : 'none',
                      }}>
                      {d.getDate()}
                    </button>
                  )
                })}
              </div>
            </div>
          ) : (
            <div className="grid grid-cols-3 gap-1.5 px-3 pb-3 pt-1">
              {MONTHS.map((name, m) => {
                const first = keyOf(new Date(viewYear, m, 1, 12))
                const future = first > todayKey
                const current = viewYear === parseKey(day).getFullYear() && m === parseKey(day).getMonth()
                return (
                  <button key={name} type="button" disabled={future}
                    onClick={() => { setView(first); setMode('days'); setFocusKey(current ? day : first) }}
                    className="mt-cal-day inline-flex items-center justify-center"
                    style={{
                      height: 44, borderRadius: 12, font: '600 13px var(--font-sans)',
                      color: current ? '#fff' : future ? 'var(--t-faint-2)' : 'var(--t-title)',
                      background: current ? 'var(--btn-primary-bg)' : 'transparent',
                      opacity: future ? 0.45 : 1,
                    }}>
                    {name.slice(0, 3)}
                  </button>
                )
              })}
            </div>
          )}

          <div className="flex items-center gap-2 px-3 py-2.5" style={{ borderTop: '1px solid var(--t-hair)' }}>
            <button type="button" onClick={() => pick(todayKey)} disabled={isToday}
              className="mt-cal-chip mt-body-sm rounded-full px-3 py-1" style={{ fontWeight: 700, color: 'var(--t-accent)' }}>
              Today
            </button>
            <button type="button" onClick={() => pick(yesterdayKey)} disabled={day === yesterdayKey}
              className="mt-cal-chip mt-body-sm rounded-full px-3 py-1" style={{ fontWeight: 600, color: 'var(--t-muted)' }}>
              Yesterday
            </button>
          </div>
        </div>,
        document.body,
      )}
    </>
  )
}
