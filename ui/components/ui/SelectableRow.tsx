//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// A pick-many row: the thing you tap to include a project, a board, a repo.
//
// It replaces a bare `<input type="checkbox">` with a label next to it. That control
// is the browser's, not ours — it renders at the OS's own size in the OS's own blue,
// ignores every token in `globals.css`, and looks a decade older than the card it
// sits inside. It is also a 13px hit target next to a 12px line of text, which is a
// fiddly thing to ask someone to hit several times in a row on the screen where they
// are first setting Meridian up.
//
// So: the whole row is the target, the selected state is carried by the surface (a
// tinted fill and an accent border, readable at a glance across a list) and not only
// by a small glyph, and the check itself is a drawn SVG that animates in.
//
// THE NATIVE INPUT IS STILL THERE, visually hidden. It keeps Tab, Space, the
// checked/mixed a11y semantics and form behaviour that a `<div onClick>` throws
// away and then has to reimplement badly. `:focus-visible` on it draws the ring on
// the row via `peer-focus-visible`, so keyboard focus stays visible.
//
// # Who calls this
// `IntegrationConnect`'s Jira and GitHub project pickers.

import type { ReactNode } from 'react'

export function SelectableRow({ checked, onChange, title, meta, disabled }: {
  checked: boolean
  onChange: () => void
  /** The name the user recognises - a project or board title. */
  title: ReactNode
  /** Secondary identifier shown as a chip on the right, e.g. a Jira key. Optional
   *  because GitHub boards have no equivalent and an empty chip is worse than none. */
  meta?: ReactNode
  disabled?: boolean
}) {
  return (
    // The hover cue is a shadow, not a background: the background is an inline style
    // (it has to be - it interpolates a token) and would win over any hover class.
    <label
      className="flex items-center gap-3 px-3 py-2.5 rounded-xl transition-all select-none hover:shadow-[0_1px_6px_-2px_rgba(0,0,0,0.18)]"
      style={{
        cursor: disabled ? 'not-allowed' : 'pointer',
        opacity: disabled ? 0.5 : 1,
        background: checked ? 'color-mix(in srgb, var(--t-accent) 9%, transparent)' : 'var(--t-card)',
        border: `1px solid ${checked ? 'color-mix(in srgb, var(--t-accent) 38%, transparent)' : 'var(--t-hair)'}`,
      }}
    >
      <input
        type="checkbox"
        className="peer sr-only"
        checked={checked}
        disabled={disabled}
        onChange={onChange}
      />

      {/* The box. Sized to the text beside it rather than to the OS default, and
          drawn in Meridian's accent so a selected row reads as ours. */}
      <span
        aria-hidden
        className="inline-flex items-center justify-center shrink-0 transition-all peer-focus-visible:outline peer-focus-visible:outline-2 peer-focus-visible:outline-offset-2"
        style={{
          width: 18, height: 18, borderRadius: 6,
          background: checked ? 'var(--btn-primary-bg)' : 'var(--t-input)',
          border: `1.5px solid ${checked ? 'var(--t-accent)' : 'var(--t-ctrl-border)'}`,
        }}
      >
        {/* Drawn, not a "✓" character: the glyph varies by platform font and sits off
            centre in the box on Windows. `pathLength` normalises the dash so the
            stroke wipes on at a consistent rate whatever the box size. */}
        <svg width="11" height="11" viewBox="0 0 12 12" fill="none" aria-hidden>
          <path
            d="M2.5 6.2 L5 8.6 L9.5 3.6"
            stroke="#fff" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"
            pathLength={1}
            style={{
              strokeDasharray: 1,
              strokeDashoffset: checked ? 0 : 1,
              transition: 'stroke-dashoffset 180ms cubic-bezier(.2,.7,.3,1)',
            }}
          />
        </svg>
      </span>

      <span className="mt-body-sm flex-1 min-w-0 truncate" style={{ color: 'var(--t-title)' }}>
        {title}
      </span>

      {meta != null && (
        <span
          className="shrink-0 text-[11px] px-1.5 py-0.5 rounded-md tabular-nums"
          style={{
            fontWeight: 600,
            letterSpacing: '0.02em',
            color: checked ? 'var(--t-accent)' : 'var(--t-faint)',
            background: checked
              ? 'color-mix(in srgb, var(--t-accent) 14%, transparent)'
              : 'var(--t-box)',
          }}
        >
          {meta}
        </span>
      )}
    </label>
  )
}
