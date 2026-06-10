// meridian — normalises screenpipe activity into structured app sessions
'use client'

import { useEffect, useState } from 'react'

// ── Time formatting ──────────────────────────────────────────────────────────
export function fmtDur(seconds: number): string {
  if (seconds < 60) return `${seconds}s`
  const m = Math.floor(seconds / 60)
  if (m < 60) return `${m}m`
  const h = Math.floor(m / 60)
  const rm = m % 60
  return rm > 0 ? `${h}h ${rm}m` : `${h}h`
}

export function fmtDurDecimal(seconds: number): string {
  if (seconds < 60) return '0:' + String(seconds).padStart(2, '0')
  const m = Math.floor(seconds / 60)
  if (m < 60) return '0:' + String(m).padStart(2, '0')
  const h = Math.floor(m / 60)
  const rm = m % 60
  return `${h}:${String(rm).padStart(2, '0')}`
}

export function fmtClock(isoOrHours: string | number): string {
  if (typeof isoOrHours === 'number') {
    const h = Math.floor(isoOrHours)
    const m = Math.round((isoOrHours - h) * 60)
    const period = h >= 12 ? 'PM' : 'AM'
    const hh = ((h + 11) % 12) + 1
    return `${hh}:${String(m).padStart(2, '0')} ${period}`
  }
  const d = new Date(isoOrHours)
  const h = d.getHours(), m = d.getMinutes()
  const period = h >= 12 ? 'PM' : 'AM'
  const hh = ((h + 11) % 12) + 1
  return `${hh}:${String(m).padStart(2, '0')} ${period}`
}

// ── Category metadata ────────────────────────────────────────────────────────
export const CATS: Record<string, { label: string; short: string }> = {
  coding:            { label: 'Coding',      short: 'Code'   },
  code_review:       { label: 'Code review', short: 'Review' },
  meeting:           { label: 'Meeting',     short: 'Meet'   },
  communication:     { label: 'Comms',       short: 'Comms'  },
  design:            { label: 'Design',      short: 'Design' },
  documentation:     { label: 'Docs',        short: 'Docs'   },
  planning:          { label: 'Planning',    short: 'Plan'   },
  deployment_devops: { label: 'DevOps',      short: 'DevOps' },
  research:          { label: 'Research',    short: 'Res'    },
  idle_personal:     { label: 'Idle',        short: 'Idle'   },
}

// ── Tracker (integration) metadata ───────────────────────────────────────────
export const PROVIDER_META: Record<string, { label: string; color: string; glyph: string }> = {
  jira:         { label: 'Jira',          color: '#2684FF', glyph: 'Ji' },
  linear:       { label: 'Linear',        color: '#5E6AD2', glyph: 'Li' },
  github:       { label: 'GitHub',        color: '#24292F', glyph: 'Gh' },
  trello:       { label: 'Trello',        color: '#0052CC', glyph: 'Tr' },
  azure_devops: { label: 'Azure DevOps',  color: '#0078D4', glyph: 'Az' },
}

export function ProviderGlyph({ provider, size = 16 }: { provider: string; size?: number }) {
  const meta = PROVIDER_META[provider]
  return (
    <span
      className="inline-flex items-center justify-center rounded shrink-0 font-mono"
      style={{
        width: size, height: size,
        fontSize: Math.max(7, size * 0.56), fontWeight: 700,
        background: (meta?.color ?? '#888') + '1A',
        color: meta?.color ?? '#888',
      }}
      aria-label={meta?.label ?? provider}
    >
      {meta?.glyph ?? provider[0]?.toUpperCase() ?? '?'}
    </span>
  )
}

// ── App glyph metadata ───────────────────────────────────────────────────────
const APP_META: Record<string, { mono: string; color: string }> = {
  'Antigravity':    { mono: 'Aᴳ', color: '#7C3AED' },
  'Google Chrome':  { mono: 'Ch', color: '#3B82F6' },
  'Terminal':       { mono: '>_', color: '#111827' },
  'DBeaver':        { mono: 'DB', color: '#1D4ED8' },
  'Claude':         { mono: 'Cl', color: '#D97757' },
  'Slack':          { mono: 'Sl', color: '#E01E5A' },
  'Zoom':           { mono: 'Zm', color: '#2D8CFF' },
  'Linear':         { mono: 'Li', color: '#5E6AD2' },
  'Figma':          { mono: 'Fg', color: '#A259FF' },
  'Notion':         { mono: 'No', color: '#111111' },
  'Spotify':        { mono: 'Sp', color: '#1DB954' },
  'Mail':           { mono: 'Ma', color: '#0EA5E9' },
  'Safari':         { mono: 'Sf', color: '#006FE5' },
  'Xcode':          { mono: 'Xc', color: '#1171A3' },
  'iTerm2':         { mono: '>_', color: '#2A2A2A' },
}

function appMeta(app: string | null | undefined) {
  if (!app) return { mono: '??', color: '#6B6A67' }
  if (APP_META[app]) return APP_META[app]
  const letters = app.trim().replace(/[^A-Za-z0-9]/g, '').slice(0, 2).toUpperCase() || '??'
  // deterministic color from name
  let h = 0
  for (let i = 0; i < app.length; i++) h = (h * 31 + app.charCodeAt(i)) & 0xffff
  const hue = h % 360
  return { mono: letters, color: `hsl(${hue}, 55%, 42%)` }
}

// ── Components ───────────────────────────────────────────────────────────────

export function CatDot({ cat, size = 6 }: { cat: string; size?: number }) {
  return (
    <span
      className={`inline-block rounded-full cat-${cat} shrink-0`}
      style={{ width: size, height: size }}
      aria-hidden
    />
  )
}

export function CatLabel({ cat, className = '' }: { cat: string; className?: string }) {
  const meta = CATS[cat] ?? CATS.idle_personal
  return (
    <span
      className={`inline-flex items-center gap-1.5 text-[11px] tracking-wide uppercase ${className}`}
      style={{ color: 'var(--ink-3)' }}
    >
      <CatDot cat={cat} />
      {meta.label}
    </span>
  )
}

export function AppGlyph({ app, size = 24, withName = false }: { app: string | null | undefined; size?: number; withName?: boolean }) {
  const meta = appMeta(app)
  return (
    <span className="inline-flex items-center gap-2">
      <span
        className="inline-flex items-center justify-center rounded-md font-mono shrink-0"
        style={{
          width: size, height: size,
          background: meta.color + '1A',
          color: meta.color,
          fontSize: Math.max(9, size * 0.42),
          fontWeight: 600,
          letterSpacing: '-0.02em',
        }}
        aria-label={app ?? undefined}
      >
        {meta.mono}
      </span>
      {withName && <span className="text-sm" style={{ color: 'var(--ink)' }}>{app}</span>}
    </span>
  )
}

/**
 * Compact display form for a task key. GitHub keys (`owner/repo#123`) overflow
 * the fixed-width key columns, so drop the owner and, if the rest is still too
 * long, ellipsize the repo while always preserving the `#123` issue number.
 * Callers showing the short form should keep the full key in a tooltip.
 */
export function shortTaskKey(keyId: string, max = 12): string {
  if (keyId.length <= max) return keyId
  const slash = keyId.indexOf('/')
  const k = slash >= 0 ? keyId.slice(slash + 1) : keyId
  if (k.length <= max) return k
  const hash = k.lastIndexOf('#')
  if (hash > 0) {
    const tail = k.slice(hash)
    const head = k.slice(0, Math.max(1, max - tail.length - 1))
    return `${head}…${tail}`
  }
  return `${k.slice(0, max - 1)}…`
}

export function TaskKey({ keyId, big = false }: { keyId?: string | null; big?: boolean }) {
  if (!keyId) return null
  const display = shortTaskKey(keyId)
  return (
    <span
      title={display === keyId ? undefined : keyId}
      className={`font-mono tracking-tight whitespace-nowrap ${big ? 'text-[12px]' : 'text-[11px]'} px-1.5 py-px rounded-[4px] tnum`}
      style={{ color: 'var(--ink)', background: 'var(--tint)', borderBottom: '1px solid var(--rule-2)' }}
    >
      {display}
    </span>
  )
}

const STATUS_META: Record<string, { label: string; dot: string }> = {
  todo:        { label: 'Todo',        dot: 'var(--ink-4)'  },
  in_progress: { label: 'In progress', dot: 'var(--accent)' },
  in_review:   { label: 'In review',   dot: '#8B5CF6'       },
  done:        { label: 'Done',        dot: 'var(--success)' },
}

export function StatusPill({ status }: { status: string }) {
  const m = STATUS_META[status] ?? STATUS_META.todo
  return (
    <span className="inline-flex items-center gap-1.5 text-[11px]" style={{ color: 'var(--ink-2)' }}>
      <span className="inline-block w-1.5 h-1.5 rounded-full" style={{ background: m.dot }} />
      {m.label}
    </span>
  )
}

export function LiveDot({ size = 8 }: { size?: number }) {
  return (
    <span className="inline-flex items-center justify-center relative" style={{ width: size, height: size }}>
      <span
        className="absolute inline-block rounded-full live-dot"
        style={{ width: size, height: size, background: 'var(--live)' }}
      />
      <span
        className="inline-block rounded-full"
        style={{ width: Math.max(3, size - 4), height: Math.max(3, size - 4), background: 'var(--live)' }}
      />
    </span>
  )
}

export function SectionHead({ kicker, title, right }: { kicker?: string; title: React.ReactNode; right?: React.ReactNode }) {
  return (
    <div className="flex items-end justify-between mb-3">
      <div>
        {kicker && (
          <p className="text-[10px] uppercase tracking-[0.16em] mb-1.5" style={{ color: 'var(--ink-3)' }}>{kicker}</p>
        )}
        <h2 className="text-[15px] font-medium" style={{ color: 'var(--ink)' }}>{title}</h2>
      </div>
      {right}
    </div>
  )
}

export function Card({
  children,
  className = '',
  as: Tag = 'div',
  style,
  ...props
}: {
  children: React.ReactNode
  className?: string
  as?: React.ElementType
  style?: React.CSSProperties
  [key: string]: unknown
}) {
  return (
    <Tag
      {...props}
      className={`rounded-xl border ${className}`}
      style={{ background: 'var(--surface)', borderColor: 'var(--rule)', ...style }}
    >
      {children}
    </Tag>
  )
}

export function ConfidenceRing({ value, size = 14 }: { value: number; size?: number }) {
  const r = size / 2 - 1.5
  const c = 2 * Math.PI * r
  const filled = c * Math.max(0, Math.min(1, value))
  const stroke = value > 0.8 ? 'var(--success)' : value > 0.5 ? 'var(--accent)' : 'var(--warn)'
  return (
    <svg width={size} height={size} viewBox={`0 0 ${size} ${size}`} aria-label={`${Math.round(value * 100)}% confidence`}>
      <circle cx={size / 2} cy={size / 2} r={r} fill="none" stroke="var(--rule-2)" strokeWidth="1.5" />
      <circle
        cx={size / 2} cy={size / 2} r={r} fill="none"
        stroke={stroke} strokeWidth="1.5" strokeLinecap="round"
        strokeDasharray={`${filled} ${c}`}
        transform={`rotate(-90 ${size / 2} ${size / 2})`}
      />
    </svg>
  )
}

export function SegBar({ segments, height = 4 }: { segments: Array<{ cat?: string; value: number; color?: string }>; height?: number }) {
  const total = segments.reduce((s, x) => s + x.value, 0) || 1
  return (
    <span className="inline-flex w-full overflow-hidden rounded-full" style={{ height, background: 'var(--rule)' }}>
      {segments.map((s, i) => (
        <span
          key={i}
          className={s.cat ? `cat-${s.cat}` : ''}
          style={{ width: `${(s.value / total) * 100}%`, background: s.color ?? undefined }}
        />
      ))}
    </span>
  )
}

export function useTick(seconds = 1): number {
  const [t, setT] = useState(0)
  useEffect(() => {
    const id = setInterval(() => setT(x => x + 1), seconds * 1000)
    return () => clearInterval(id)
  }, [seconds])
  return t
}
