//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

import { ProviderIcon } from './ProviderIcon'
import { BRAND_ICONS } from '@/lib/brand-icons'
import { useAppIconUrl } from '@/lib/app-icons'

// ── Time formatting ──────────────────────────────────────────────────────────
export function fmtDur(seconds: number): string {
  if (seconds < 60) return `${seconds}s`
  const m = Math.floor(seconds / 60)
  if (m < 60) return `${m}m`
  const h = Math.floor(m / 60)
  const rm = m % 60
  return rm > 0 ? `${h}h ${rm}m` : `${h}h`
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
      className="inline-flex items-center justify-center rounded shrink-0"
      style={{
        width: size, height: size,
        background: (meta?.color ?? '#888') + '1A',
      }}
      aria-label={meta?.label ?? provider}
    >
      {meta
        ? <ProviderIcon provider={provider} size={Math.round(size * 0.62)} />
        : <span className="font-mono" style={{ fontSize: Math.max(7, size * 0.56), fontWeight: 700, color: '#888' }}>
            {provider[0]?.toUpperCase() ?? '?'}
          </span>}
    </span>
  )
}

// ── App glyph metadata ───────────────────────────────────────────────────────
// Apps with a real vector wordmark live in BRAND_ICONS (lib/brand-icons.ts)
// and take priority in AppGlyph below. This table is only the monogram
// fallback for apps with no redistributable brand mark — Apple's own system
// apps (Terminal, Mail) and unreleased/unlisted tools (Antigravity).
const APP_META: Record<string, { mono: string; color: string }> = {
  'Antigravity':    { mono: 'Aᴳ', color: '#7C3AED' },
  'Terminal':       { mono: '>_', color: '#111827' },
  'Mail':           { mono: 'Ma', color: '#0EA5E9' },
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

export function AppGlyph({ app, size = 24, withName = false }: { app: string | null | undefined; size?: number; withName?: boolean }) {
  // Real icon (extracted from the installed .app bundle) wins once resolved;
  // the brand wordmark / letter monogram render immediately in the meantime
  // (and stay as the permanent fallback for an app we can't resolve).
  const iconUrl = useAppIconUrl(app)
  const brand = app ? BRAND_ICONS[app] : undefined
  const meta = appMeta(app)
  const color = brand?.hex ?? meta.color
  return (
    <span className="inline-flex items-center gap-2">
      <span
        className="inline-flex items-center justify-center rounded-md shrink-0 overflow-hidden"
        style={{ width: size, height: size, background: iconUrl ? 'transparent' : color + '1A' }}
        aria-label={app ?? undefined}
      >
        {iconUrl ? (
          // eslint-disable-next-line @next/next/no-img-element -- local asset:// URL, not a Next-optimizable remote image
          <img src={iconUrl} alt="" width={size} height={size} style={{ objectFit: 'contain' }} />
        ) : brand ? (
          <svg viewBox={brand.viewBox} width={size * 0.58} height={size * 0.58} fill={brand.hex} aria-hidden="true">
            <path d={brand.path} />
          </svg>
        ) : (
          <span
            className="font-mono"
            style={{ color, fontSize: Math.max(9, size * 0.42), fontWeight: 600, letterSpacing: '-0.02em' }}
          >
            {meta.mono}
          </span>
        )}
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

// Status names are now dynamic — they come straight from the user's tracker
// ("In Review", "QA", "Ready for Deploy", …), so we render the raw name verbatim
// rather than collapsing it into a fixed set. Colour is driven by `isTerminal`
// (the resolved "is this done?" signal). When a caller has only the raw string
// (e.g. the command bar), we infer terminality from a keyword fallback.
const TERMINAL_KEYWORDS = ['done', 'complete', 'closed', 'resolved', 'shipped', 'merged', 'deployed', 'released', 'archived', 'cancel']

function humanizeStatus(raw: string): string {
  const s = raw.replace(/_/g, ' ').trim()
  return s ? s.charAt(0).toUpperCase() + s.slice(1) : ''
}

function looksTerminal(raw: string): boolean {
  const lower = raw.toLowerCase()
  return TERMINAL_KEYWORDS.some(kw => lower.includes(kw))
}

export function StatusPill({ status, isTerminal }: { status: string; isTerminal?: boolean }) {
  if (!status) return null
  const terminal = isTerminal ?? looksTerminal(status)
  const dot = terminal ? 'var(--success)' : 'var(--accent)'
  return (
    <span className="inline-flex items-center gap-1.5 text-[11px]" style={{ color: 'var(--ink-2)' }}>
      <span className="inline-block w-1.5 h-1.5 rounded-full" style={{ background: dot }} />
      {humanizeStatus(status)}
    </span>
  )
}

/**
 * Shared badge/pill shape — consolidates the two patterns hand-rolled across
 * timeline components: `chip` (small rounded corners, mono `.mt-chip` text —
 * the outlined data-label badges on cards) and `pill` (rounded-full, used for
 * filled status indicators like "Paused"). `tone` controls the fill:
 * `outline` (default, border only), `filled` (color-mix background + border,
 * for a colored status pill), `ghost` (neutral `bg-wrap` fill with a
 * hairline border in `color`).
 */
export function Badge({
  children,
  color = 'var(--t-muted)',
  borderColor,
  variant = 'chip',
  tone = 'outline',
  dot = false,
  dotClassName = '',
  as: Tag = 'span',
  className = '',
  style,
  ...props
}: {
  children: React.ReactNode
  color?: string
  // Outline tone only — `filled`/`ghost` compute their own border from
  // `color`/tone and ignore this. Defaults to `color` — override for the
  // neutral chips whose border reads as a plain hairline (`--t-hair`) rather
  // than tinted to match the text.
  borderColor?: string
  variant?: 'chip' | 'pill'
  tone?: 'outline' | 'filled' | 'ghost'
  dot?: boolean
  // Extra class on the leading dot — e.g. `live-dot` for a blinking pulse.
  dotClassName?: string
  as?: React.ElementType
  className?: string
  style?: React.CSSProperties
  [key: string]: unknown
}) {
  const shape = variant === 'pill' ? 'rounded-full' : 'rounded'
  const pad = variant === 'pill' ? 'px-2 py-1' : 'px-1.5 py-0.5'
  const outlineColor = borderColor ?? color
  const border = tone === 'ghost' ? 'none'
    : tone === 'filled' ? `1px solid color-mix(in srgb, ${color} 24%, transparent)`
    : `1px solid ${outlineColor}`
  const background = tone === 'filled' ? `color-mix(in srgb, ${color} 12%, transparent)`
    : tone === 'ghost' ? 'var(--t-wrap)' : undefined
  // `inline-flex` only when a dot is actually rendered — a dot-less badge
  // stays plain `inline` so its baseline matches the hand-rolled markup it
  // replaced exactly (inline-flex's baseline calculation can shift vertical
  // alignment by a pixel or two next to surrounding text).
  const display = dot ? 'inline-flex items-center gap-1.5' : 'inline'
  return (
    <Tag
      {...props}
      className={`mt-chip ${display} shrink-0 ${shape} ${pad} ${className}`}
      style={{ color, border, background, ...style }}
    >
      {dot && <span className={`inline-block w-1.5 h-1.5 rounded-full shrink-0 ${dotClassName}`} style={{ background: color }} aria-hidden="true" />}
      {children}
    </Tag>
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

