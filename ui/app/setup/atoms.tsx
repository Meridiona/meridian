//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// Shared presentational atoms for the setup wizard (ported from the design's
// wizard-atoms.jsx). Retokenized onto the product's own CSS variables
// (globals.css) so the wizard matches the dashboard it hands off to — the
// design's standalone token block is dropped in favour of --accent/--ink/etc.

import type { CSSProperties, ReactNode } from 'react'

// ── Rounded mono glyph mark (apps / integrations) ────────────────────────────
export function Mark({ mono, color, size = 34, radius }: {
  mono: string; color: string; size?: number; radius?: number
}) {
  return (
    <span className="inline-flex items-center justify-center font-mono shrink-0"
      style={{
        width: size, height: size, borderRadius: radius ?? size * 0.28,
        background: color + '16', color,
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
  return null
}

// ── Check + spinner ──────────────────────────────────────────────────────────
export function Check({ size = 14, color = 'currentColor', w = 2 }: { size?: number; color?: string; w?: number }) {
  return (<svg width={size} height={size} viewBox="0 0 16 16" fill="none"
    stroke={color} strokeWidth={w} strokeLinecap="round" strokeLinejoin="round" aria-hidden>
    <path d="M3.5 8.5 6.5 11.5 12.5 5" /></svg>)
}

export function Spinner({ size = 14, width = 1.6, color = 'var(--accent)' }: { size?: number; width?: number; color?: string }) {
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
    primary: { background: 'var(--accent)', color: '#fff', boxShadow: '0 1px 2px rgba(0,0,0,.12)' },
    secondary: { background: 'var(--surface)', color: 'var(--ink)', borderColor: 'var(--rule-2)' },
    ghost: { background: 'transparent', color: 'var(--ink-3)' },
    soft: { background: 'var(--accent-soft)', color: 'var(--accent)' },
  }
  const dim: CSSProperties | null = disabled ? { opacity: 0.4, filter: 'saturate(.6)' } : null
  return (
    <button title={title} onClick={disabled ? undefined : onClick} disabled={disabled}
      style={{ ...base, ...skins[variant], ...dim, ...style }}
      onMouseEnter={(e) => { if (disabled) return
        if (variant === 'primary') e.currentTarget.style.filter = 'brightness(1.06)'
        else e.currentTarget.style.background = 'var(--surface-2)' }}
      onMouseLeave={(e) => { if (disabled) return
        e.currentTarget.style.filter = 'none'
        if (variant !== 'primary') e.currentTarget.style.background = String(skins[variant].background) }}>
      {children}
    </button>
  )
}

// ── Progress bar ─────────────────────────────────────────────────────────────
export function Bar({ pct, height = 6, track = 'var(--rule)', fill = 'var(--accent)' }: {
  pct: number; height?: number; track?: string; fill?: string
}) {
  return (
    <div style={{ height, borderRadius: 99, background: track, overflow: 'hidden', width: '100%' }}>
      <div style={{ width: `${Math.max(0, Math.min(100, pct))}%`, height: '100%', borderRadius: 99,
        background: fill, transition: 'width .25s linear' }} />
    </div>
  )
}

// ── Kicker / eyebrow label ───────────────────────────────────────────────────
export function Kicker({ children, color = 'var(--ink-3)', style }: { children: ReactNode; color?: string; style?: CSSProperties }) {
  return (<p className="font-mono" style={{ fontSize: 10, letterSpacing: '0.18em', textTransform: 'uppercase', color, ...style }}>{children}</p>)
}

// ── Bordered row tile (permission + integration + runtime rows) ──────────────
export function Row({ children, tone = 'surface', style }: {
  children: ReactNode; tone?: 'surface' | 'tint' | 'soft'; style?: CSSProperties
}) {
  const bg = tone === 'tint' ? 'var(--tint)' : tone === 'soft' ? 'var(--surface-2)' : 'var(--surface)'
  return (
    <div style={{
      display: 'flex', alignItems: 'center', gap: 13, padding: '13px 15px',
      border: '0.5px solid var(--rule-2)', borderRadius: 13, background: bg, ...style,
    }}>{children}</div>
  )
}
