//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// Shared presentational atoms for the setup wizard (ported from the design's
// wizard-atoms.jsx). Retokenized onto the product's own CSS variables
// (globals.css) so the wizard matches the dashboard it hands off to — the
// design's standalone token block is dropped in favour of --accent/--ink/etc.

import type { CSSProperties, ReactNode } from 'react'

/**
 * Display-heading treatment shared by every wizard hero + card header — the
 * setup wizard's step headers and Welcome/Completion heroes, and the uninstall
 * wizard's header. One voice with the timeline UI: SF Pro (var(--font-sans)),
 * not the Instrument Serif these headings used to be set in.
 *
 * Lives here rather than in each page so the two wizards can't drift apart —
 * they previously carried three verbatim copies of this object, which is
 * exactly the divergence that made uninstall keep its serif heading after
 * setup had moved off it.
 *
 * Carries the weight and tracking too, not just the family: every call site was
 * overriding the old 700/-.02em with 750/-.03em, so those defaults were dead
 * values that described no heading actually rendered. Call sites now set only
 * what genuinely varies - fontSize, lineHeight, and color.
 */
export const DISPLAY: CSSProperties = {
  fontFamily: 'var(--font-sans)',
  fontWeight: 750,
  letterSpacing: '-.03em',
}

// ── Brand mark (the real app icon, not a colored dot) ────────────────────────
// Renders `ui/public/meridian-logo.png`, cropped in to the mark itself (same
// idea as the tray popover's `.brand-mark`, tray/src/style.css). The mark's
// pixel bounding box is ~60.5% of the 512x512 canvas BUT is not centered on
// it - it sits ~17px above true centre (measured directly from the PNG:
// content spans x:100-409, y:85-394). backgroundPosition:center crops around
// the canvas's geometric centre, not the mark's, so the old 166% zoom (sized
// to exactly match the bbox width) clipped the top of the outer rings. 146%
// is the largest zoom that still keeps the full off-centre bbox inside the
// crop window on every side.
export function Logo({ size = 30 }: { size?: number }) {
  return (
    <span aria-hidden="true" className="shrink-0" style={{
      width: size, height: size, borderRadius: size / 3.6,
      backgroundImage: 'url(/meridian-logo.png)',
      backgroundSize: '146% 146%', backgroundPosition: 'center', backgroundRepeat: 'no-repeat',
    }} />
  )
}

// ── OS permission-toggle preview ──────────────────────────────────────────────
// A small illustrative mock of "Meridian" as it appears, switched on, in the
// OS's own permissions list — not a captured screenshot (no access to either
// OS's real UI here), reconstructed from the real macOS System Settings panes
// (Privacy & Security → Accessibility / Screen & System Audio Recording -
// Notifications uses the same list-of-apps-with-toggles structure on macOS
// too, so it reuses the same template) to read as "this is what you'll see"
// rather than a generic illustration. Windows never shows Accessibility/Screen
// Recording at all (no TCC consent analogue there), so it only ever needs the
// simpler single-row preview for Notifications.
//
// Font sizes here run a couple of points bigger than a literal 1:1 shrink of
// the real System Settings pane would use - at this card's actual on-screen
// size a true-to-scale reproduction (8-9px type) read as barely legible, not
// authentically tiny.
const MAC_PANE_COPY: Record<'accessibility' | 'screen' | 'notifications', { title: string; blurb: string; others: string[] }> = {
  accessibility: {
    title: 'Accessibility',
    blurb: 'Allow the applications below to control your computer.',
    others: ['Terminal', 'Google Chrome'],
  },
  screen: {
    title: 'Screen & System Audio Recording',
    blurb: 'Allow the applications below to record the content of your screen.',
    others: ['Terminal', 'Google Chrome'],
  },
  notifications: {
    title: 'Notifications',
    blurb: 'Allow the applications below to send notifications.',
    others: ['Terminal', 'Google Chrome'],
  },
}

function MacToggleOn() {
  return (
    <span aria-hidden="true" className="flex items-center shrink-0" style={{
      width: 32, height: 19, borderRadius: 99, background: '#0A84FF', padding: 2, justifyContent: 'flex-end',
    }}>
      <span style={{ width: 15, height: 15, borderRadius: 99, background: '#fff', boxShadow: '0 1px 2px rgba(0,0,0,.25)' }} />
    </span>
  )
}

/** Reconstructed macOS System Settings pane (header chevrons, title, blurb,
 *  and the app list) - used for all three permissions on macOS. Every OTHER
 *  app in the list is a blurred, generic placeholder (never a real app's
 *  name/icon - not what was in the reference screenshots, genericised on
 *  purpose, and the SAME two placeholders for every permission so all three
 *  panes have identical structure); the Meridian row is the one thing left
 *  sharp and highlighted so the eye lands on it immediately.
 *
 *  Fills the full height of its container (`height:'100%'`, the app list
 *  `flex:1`) rather than sizing to content - the three permission cards are
 *  equal height (`items-stretch` in PermissionsBody), so without this the
 *  pane itself would end up a different visible size in each card depending
 *  on how much text its title/blurb wrapped to. */
function MacSettingsPanePreview({ permissionId }: { permissionId: 'accessibility' | 'screen' | 'notifications' }) {
  const copy = MAC_PANE_COPY[permissionId]
  return (
    <div className="w-full flex flex-col" style={{
      height: '100%', borderRadius: 10, overflow: 'hidden',
      border: '0.5px solid var(--t-card-border)', background: '#fff',
    }}>
      <div className="flex items-center shrink-0" style={{ gap: 7, padding: '6px 10px', background: '#F0F0F1' }}>
        <span style={{
          width: 16, height: 16, borderRadius: 99, background: '#fff', border: '0.5px solid #d2d2d7',
          display: 'flex', alignItems: 'center', justifyContent: 'center', fontSize: 10, color: '#1d1d1f', lineHeight: 1,
        }}>‹</span>
        <span style={{
          width: 16, height: 16, borderRadius: 99, background: '#fff', border: '0.5px solid #e5e5e7',
          display: 'flex', alignItems: 'center', justifyContent: 'center', fontSize: 10, color: '#c7c7cc', lineHeight: 1,
        }}>›</span>
        <span style={{ fontSize: 12.5, fontWeight: 700, color: '#1d1d1f', marginLeft: 2 }}>{copy.title}</span>
      </div>

      <p className="shrink-0" style={{ fontSize: 10.5, lineHeight: 1.35, color: '#48484a', padding: '5px 10px 4px' }}>{copy.blurb}</p>

      <div className="flex flex-col" style={{ flex: 1, margin: '2px 8px 7px', borderRadius: 8, overflow: 'hidden', background: '#F0F0F1' }}>
        {copy.others.map((name) => (
          <div key={name} className="flex items-center shrink-0" style={{
            gap: 8, padding: '5px 10px', borderBottom: '0.5px solid rgba(0,0,0,.07)',
            filter: 'blur(2px)', opacity: 0.5,
          }}>
            <span style={{ width: 16, height: 16, borderRadius: 4, background: '#c7c7cc', flexShrink: 0 }} />
            <span style={{ flex: 1, fontSize: 10.5, color: '#1d1d1f' }}>{name}</span>
            <MacToggleOn />
          </div>
        ))}
        {/* Meridian - sharp and tinted so it visually "pops" out from the
           blurred rows above it. A bigger icon/text/padding (not a transform
           scale) makes it read as "larger" without risking an overflow past
           the pane's own rounded corners - a scale() here used to spill the
           toggle out past the card edge. flex:1 so it absorbs whatever height
           the (equal-length, but wrap-sensitive) blurred rows above don't. */}
        <div className="flex items-center" style={{
          flex: 1, gap: 9, padding: '7px 11px', position: 'relative', zIndex: 1, borderRadius: 8,
          background: 'color-mix(in srgb, var(--color-state-proposal) 11%, #fff)',
          boxShadow: 'inset 0 0 0 1.5px color-mix(in srgb, var(--color-state-proposal) 50%, transparent)',
        }}>
          <Logo size={19} />
          <span style={{ flex: 1, fontSize: 13, fontWeight: 700, color: '#1d1d1f' }}>Meridian</span>
          <MacToggleOn />
        </div>
      </div>

      <div className="flex items-center shrink-0" style={{ gap: 6, padding: '0 8px 6px' }}>
        {['+', '−'].map((glyph) => (
          <span key={glyph} style={{
            width: 17, height: 17, borderRadius: 3, background: '#F0F0F1', border: '0.5px solid #e5e5e7',
            display: 'flex', alignItems: 'center', justifyContent: 'center', fontSize: 11, color: '#48484a', lineHeight: 1,
          }}>{glyph}</span>
        ))}
      </div>
    </div>
  )
}

/** The simple single-row preview - used for Notifications on Windows only now
 *  (macOS Notifications uses `MacSettingsPanePreview` like the other two).
 *  Also fills its container height, centering the row vertically, so it
 *  matches whatever height the (equal-height, `items-stretch`) sibling cards
 *  end up at. */
function SimpleTogglePreview({ os }: { os: 'macos' | 'windows' }) {
  const isWin = os === 'windows'
  return (
    <div className="flex flex-col items-center justify-center" style={{ gap: 6, width: '100%', height: '100%' }}>
      <div className="flex items-center" style={{
        gap: 10, width: '100%', padding: '9px 12px',
        borderRadius: isWin ? 6 : 10,
        background: 'var(--t-box)', border: '0.5px solid var(--t-card-border)',
      }}>
        <Logo size={22} />
        <div style={{ flex: 1, minWidth: 0 }}>
          <p style={{ fontSize: 13, fontWeight: 500, color: 'var(--t-title)', lineHeight: 1.2 }}>Meridian</p>
          {/* Windows Settings rows carry a subtext line (publisher/path) the
             macOS list doesn't - the detail that makes each read as its own
             OS rather than a reskinned copy of the other. */}
          {isWin && <p style={{ fontSize: 10.5, color: 'var(--t-faint)', lineHeight: 1.2, marginTop: 1 }}>Meridian.exe</p>}
        </div>
        {/* macOS toggle: fully pill-shaped track, system blue when on.
           Windows toggle: flatter rounded-rect track, accent blue when on -
           the two OSes' actual toggle shapes differ and this keeps that. */}
        <span aria-hidden="true" className="flex items-center shrink-0" style={{
          width: isWin ? 42 : 34, height: 21, borderRadius: isWin ? 10 : 99,
          background: isWin ? '#0067C0' : '#0A84FF',
          padding: 2, justifyContent: 'flex-end',
        }}>
          <span style={{ width: 17, height: 17, borderRadius: 99, background: '#fff', boxShadow: '0 1px 2px rgba(0,0,0,.25)' }} />
        </span>
      </div>
      <p style={{ fontSize: 10.5, color: 'var(--t-faint)' }}>What you&apos;ll see in {isWin ? 'Windows Settings' : 'System Settings'}</p>
    </div>
  )
}

export function PermissionPreview({ os, permissionId }: {
  os: 'macos' | 'windows'
  permissionId: 'accessibility' | 'screen' | 'notifications'
}) {
  if (os === 'macos') {
    return <MacSettingsPanePreview permissionId={permissionId} />
  }
  return <SimpleTogglePreview os={os} />
}

// ── Rounded mono glyph mark (apps / integrations) ────────────────────────────
export function Mark({ mono, color, size = 34, radius }: {
  mono: string; color: string; size?: number; radius?: number
}) {
  // `color` may be a hex literal (#rrggbb) or a CSS variable (var(--color-state-proposal)).
  // Appending an alpha suffix only works for hex; for anything else use
  // color-mix so the tint renders instead of becoming an invalid value.
  const background = color.startsWith('#')
    ? `${color}16`
    : `color-mix(in srgb, ${color} 9%, transparent)`
  return (
    <span className="inline-flex items-center justify-center font-mono shrink-0"
      style={{
        width: size, height: size, borderRadius: radius ?? size * 0.28,
        background, color,
        fontSize: Math.max(11, size * 0.4), fontWeight: 600, letterSpacing: '-0.02em',
      }}>{mono}</span>
  )
}

// ── Monoline permission icons (basic shapes only) ────────────────────────────
export function PermIcon({ icon, size = 18 }: { icon: string; size?: number }) {
  const c = { width: size, height: size }
  const s = { fill: 'none', stroke: 'currentColor', strokeWidth: 1.4, strokeLinecap: 'round', strokeLinejoin: 'round' } as const
  if (icon === 'access')
    return (<svg viewBox="0 0 18 18" style={c}><circle cx="9" cy="5" r="2.2" {...s} /><path d="M3.5 15c0-3 2.5-5 5.5-5s5.5 2 5.5 5" {...s} /></svg>)
  if (icon === 'screen')
    return (<svg viewBox="0 0 18 18" style={c}><rect x="2.5" y="3.5" width="13" height="8.5" rx="1.4" {...s} /><path d="M6.5 15h5M9 12v3" {...s} /></svg>)
  if (icon === 'power')
    return (<svg viewBox="0 0 18 18" style={c}><path d="M9 3v6" {...s} /><path d="M5.5 5.5a5 5 0 1 0 7 0" {...s} /></svg>)
  if (icon === 'bell')
    return (<svg viewBox="0 0 18 18" style={c}><path d="M9 3a4 4 0 0 1 4 4v2.5l1.2 2.3H3.8L5 9.5V7a4 4 0 0 1 4-4z" {...s} /><path d="M7.6 14.5a1.5 1.5 0 0 0 2.8 0" {...s} /></svg>)
  if (icon === 'mail')
    return (<svg viewBox="0 0 18 18" style={c}><rect x="2" y="4" width="14" height="10" rx="1.6" {...s} /><path d="M2.6 5 9 10 15.4 5" {...s} /></svg>)
  if (icon === 'shield')
    return (<svg viewBox="0 0 18 18" style={c}><path d="M9 2.5 15 4.5v3.8c0 4-2.6 6.3-6 7-3.4-.7-6-3-6-7V4.5Z" {...s} /><path d="M6.4 9 8.2 10.8 11.7 6.8" {...s} /></svg>)
  return null
}

// ── Check + spinner ──────────────────────────────────────────────────────────
export function Check({ size = 14, color = 'currentColor', w = 2 }: { size?: number; color?: string; w?: number }) {
  return (<svg width={size} height={size} viewBox="0 0 16 16" fill="none"
    stroke={color} strokeWidth={w} strokeLinecap="round" strokeLinejoin="round" aria-hidden>
    <path d="M3.5 8.5 6.5 11.5 12.5 5" /></svg>)
}

export function Spinner({ size = 14, width = 1.6, color = 'var(--color-state-proposal)' }: { size?: number; width?: number; color?: string }) {
  return (
    <span className="mer-spin inline-block" style={{ width: size, height: size }} aria-hidden>
      <svg width={size} height={size} viewBox="0 0 16 16" fill="none">
        <circle cx="8" cy="8" r="6.2" stroke="currentColor" strokeOpacity="0.18" strokeWidth={width} />
        <path d="M8 1.8a6.2 6.2 0 0 1 6.2 6.2" stroke={color} strokeWidth={width} strokeLinecap="round" />
      </svg>
    </span>
  )
}

// ── Button ───────────────────────────────────────────────────────────────────
export function Btn({ children, variant = 'primary', size = 'md', disabled, onClick, style, title }: {
  children: ReactNode
  variant?: 'primary' | 'secondary' | 'ghost' | 'soft'
  size?: 'sm' | 'md'
  disabled?: boolean
  onClick?: () => void
  style?: CSSProperties
  title?: string
}) {
  const base: CSSProperties = {
    display: 'inline-flex', alignItems: 'center', justifyContent: 'center', gap: 7,
    padding: size === 'sm' ? '6px 12px' : '9px 18px', fontSize: size === 'sm' ? 12 : 13,
    fontWeight: 500, borderRadius: 9, lineHeight: 1, whiteSpace: 'nowrap',
    transition: 'all .14s', cursor: disabled ? 'default' : 'pointer',
    border: '0.5px solid transparent',
  }
  const skins: Record<string, CSSProperties> = {
    primary: { background: 'var(--color-state-proposal)', color: '#fff', boxShadow: '0 1px 2px rgba(0,0,0,.12)' },
    secondary: { background: 'var(--t-card)', color: 'var(--t-title)', borderColor: 'var(--t-card-border)' },
    ghost: { background: 'transparent', color: 'var(--t-faint)' },
    soft: { background: 'color-mix(in srgb, var(--color-state-proposal) 12%, transparent)', color: 'var(--color-state-proposal)' },
  }
  const dim: CSSProperties | null = disabled ? { opacity: 0.4, filter: 'saturate(.6)' } : null
  return (
    <button title={title} onClick={disabled ? undefined : onClick} disabled={disabled}
      style={{ ...base, ...skins[variant], ...dim, ...style }}
      onMouseEnter={(e) => { if (disabled) return
        if (variant === 'primary') e.currentTarget.style.filter = 'brightness(1.06)'
        else e.currentTarget.style.background = 'var(--t-box)' }}
      onMouseLeave={(e) => { if (disabled) return
        e.currentTarget.style.filter = 'none'
        if (variant !== 'primary') e.currentTarget.style.background = String(skins[variant].background) }}>
      {children}
    </button>
  )
}

// ── Progress bar ─────────────────────────────────────────────────────────────
export function Bar({ pct, height = 6, track = 'var(--t-hair)', fill = 'var(--color-state-proposal)' }: {
  pct: number; height?: number; track?: string; fill?: string
}) {
  return (
    <div style={{ height, borderRadius: 99, background: track, overflow: 'hidden', width: '100%' }}>
      <div style={{ width: `${Math.max(0, Math.min(100, pct))}%`, height: '100%', borderRadius: 99,
        background: fill, transition: 'width .25s linear' }} />
    </div>
  )
}

// ── Semi-circular memory gauge ───────────────────────────────────────────────
// Distinct on purpose from `Bar` above (straight, used for download progress):
// this reads as a capacity dial rather than a loading indicator, so the two
// never get confused for one another. Drawn as two arcs meeting at `pct` —
// amber for the footprint Meridian expects to use, green for the unified
// memory left over — rather than a single fill against a neutral track.
export function MemoryGauge({ pct, size = 96, strokeWidth = 9 }: {
  pct: number; size?: number; strokeWidth?: number
}) {
  const r = size / 2 - strokeWidth
  const cx = size / 2
  const cy = size / 2
  const clamped = Math.max(0, Math.min(100, pct))
  const splitAngle = 180 + (clamped / 100) * 180
  const polar = (angleDeg: number) => {
    const rad = (angleDeg * Math.PI) / 180
    return { x: cx + r * Math.cos(rad), y: cy + r * Math.sin(rad) }
  }
  const arc = (startAngle: number, endAngle: number) => {
    const start = polar(startAngle)
    const end = polar(endAngle)
    const largeArc = endAngle - startAngle > 180 ? 1 : 0
    return `M ${start.x} ${start.y} A ${r} ${r} 0 ${largeArc} 1 ${end.x} ${end.y}`
  }
  return (
    <svg width={size} height={size / 2 + strokeWidth} viewBox={`0 0 ${size} ${size / 2 + strokeWidth}`}>
      <path d={arc(splitAngle, 360)} fill="none" stroke="var(--color-state-approved)"
        strokeWidth={strokeWidth} strokeLinecap="round" />
      <path d={arc(180, splitAngle)} fill="none" stroke="var(--color-state-pending)"
        strokeWidth={strokeWidth} strokeLinecap="round" />
    </svg>
  )
}

// ── Kicker / eyebrow label ───────────────────────────────────────────────────
// Same class + accent color as the Settings → Intelligence "Your AI" eyebrow
// (IntelligenceSection.tsx) — the one shared eyebrow style, not a wizard-only one.
export function Kicker({ children, color = 'var(--color-state-proposal)', style }: { children: ReactNode; color?: string; style?: CSSProperties }) {
  return (<p className="mt-label" style={{ color, ...style }}>{children}</p>)
}

// ── Bordered row tile (permission + integration + runtime rows) ──────────────
export function Row({ children, tone = 'surface', style }: {
  children: ReactNode; tone?: 'surface' | 'tint' | 'soft'; style?: CSSProperties
}) {
  const bg = tone === 'tint' ? 'color-mix(in srgb, var(--color-state-proposal) 8%, transparent)' : tone === 'soft' ? 'var(--t-box)' : 'var(--t-card)'
  return (
    <div style={{
      display: 'flex', alignItems: 'center', gap: 13, padding: '13px 15px',
      border: '0.5px solid var(--t-card-border)', borderRadius: 13, background: bg, ...style,
    }}>{children}</div>
  )
}
