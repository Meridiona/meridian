//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Interactive "time by category" donut chart — the Overview panel's
// Time-by-app sibling, but aggregates get_today sessions by `cat` (coding,
// meeting, research, …) instead of `app`, and renders as a hoverable SVG
// donut + legend instead of horizontal bars (a pie/donut reads distribution
// share — "how much of today" — more directly than stacked bars for a small,
// mutually-exclusive category set). Segment/dot colors are the exact hex
// values behind the existing `.cat-<key>` classes (globals.css) so this
// matches every other category dot in the app (TimelineColumn's hour-row
// dots, CatDot) rather than inventing a new palette — SVG `stroke` can't
// consume a CSS class the way a `background` can, so the hexes are mirrored
// here; keep them in sync if globals.css's cat-* values ever change.
//
// `agentSeconds` (today.agent_s, the Overview panel's "Agent time" stat) is
// added into the "Coding" slice rather than shown as its own category — from
// a user's perspective, coding-agent tool time IS coding, just delegated;
// splitting it into a second slice/label made the chart read as if two
// unrelated numbers disagreed. The Overview panel's separate "Agent time"
// card still shows the agent-only breakdown (incl. how much ran
// autonomously) for anyone who wants that detail.

'use client'

import { useMemo, useState } from 'react'
import { fmtDur, CATS } from '@/components/atoms'
import type { TodaySession } from '@/lib/api-types'

const CAT_HEX: Record<string, string> = {
  coding:             '#3B6FE0',
  code_review:        '#7C3AED',
  meeting:            '#D97706',
  communication:      '#059669',
  design:             '#DB2777',
  documentation:      '#0891B2',
  planning:           '#C4822A',
  deployment_devops:  '#DC2626',
  research:           '#4F46E5',
  idle_personal:      '#78716C',
}
const FALLBACK_HEX = '#8B5CF6'
/** Aggregate sessions into category totals, descending. */
export function categoryTotals(sessions: TodaySession[]): Array<{ cat: string; seconds: number }> {
  const by = new Map<string, number>()
  for (const s of sessions) {
    if (!s.cat) continue
    by.set(s.cat, (by.get(s.cat) ?? 0) + s.dur)
  }
  return Array.from(by.entries())
    .map(([cat, seconds]) => ({ cat, seconds }))
    .sort((a, b) => b.seconds - a.seconds)
}

interface Row { cat: string; seconds: number; label: string; color: string }

/** categoryTotals, with `agentSeconds` (today.agent_s) folded into "coding" — see file header. */
export function categoryRows(sessions: TodaySession[], agentSeconds = 0): Row[] {
  const totals = categoryTotals(sessions)
  const merged = agentSeconds > 0
    ? (totals.some(r => r.cat === 'coding')
        ? totals.map(r => r.cat === 'coding' ? { ...r, seconds: r.seconds + agentSeconds } : r)
        : [...totals, { cat: 'coding', seconds: agentSeconds }])
    : totals
  return merged
    .map(r => ({ ...r, label: CATS[r.cat]?.label ?? r.cat, color: CAT_HEX[r.cat] ?? FALLBACK_HEX }))
    .sort((a, b) => b.seconds - a.seconds)
}

const SIZE = 116
const STROKE = 15
const R = (SIZE - STROKE) / 2
const CIRC = 2 * Math.PI * R

export function TimeByCategory({ sessions, agentSeconds = 0, limit = 7 }: {
  sessions: TodaySession[]
  agentSeconds?: number
  limit?: number
}) {
  const rows = useMemo(() => categoryRows(sessions, agentSeconds).slice(0, limit), [sessions, agentSeconds, limit])
  const total = rows.reduce((a, r) => a + r.seconds, 0)
  const [hover, setHover] = useState<string | null>(null)

  if (rows.length === 0) {
    return <p className="mt-body-sm italic" style={{ color: 'var(--t-faint-2)' }}>No category activity yet.</p>
  }

  const active = hover ? rows.find(r => r.cat === hover) : null
  let cumulative = 0

  return (
    <div className="flex items-center gap-5">
      <div className="relative shrink-0" style={{ width: SIZE, height: SIZE }}>
        <svg width={SIZE} height={SIZE} viewBox={`0 0 ${SIZE} ${SIZE}`} style={{ transform: 'rotate(-90deg)' }}>
          <circle cx={SIZE / 2} cy={SIZE / 2} r={R} fill="none" stroke="var(--t-track)" strokeWidth={STROKE} />
          {rows.map(row => {
            const frac = total > 0 ? row.seconds / total : 0
            const dash = frac * CIRC
            const offset = -(cumulative / total) * CIRC
            cumulative += row.seconds
            const dimmed = hover !== null && hover !== row.cat
            return (
              <circle
                key={row.cat}
                cx={SIZE / 2} cy={SIZE / 2} r={R} fill="none"
                stroke={row.color}
                strokeWidth={hover === row.cat ? STROKE + 3 : STROKE}
                strokeDasharray={`${dash} ${CIRC - dash}`}
                strokeDashoffset={offset}
                opacity={dimmed ? 0.35 : 1}
                style={{ cursor: 'pointer', transition: 'opacity .15s, stroke-width .15s' }}
                onMouseEnter={() => setHover(row.cat)}
                onMouseLeave={() => setHover(null)}
              />
            )
          })}
        </svg>
        {/* center label — active segment on hover, else the day's total */}
        <div className="absolute inset-0 flex flex-col items-center justify-center pointer-events-none text-center px-2">
          <p className="mt-stat" style={{ color: active ? active.color : 'var(--t-title)', fontSize: 15 }}>
            {fmtDur(active ? active.seconds : total)}
          </p>
          <p className="mt-label truncate" style={{ color: 'var(--t-faint)', fontSize: 8.5, maxWidth: SIZE - 24 }}>
            {active ? active.label : 'Total'}
          </p>
        </div>
      </div>

      <div className="flex-1 min-w-0 space-y-1.5">
        {rows.map(row => {
          const pct = total > 0 ? Math.round((row.seconds / total) * 100) : 0
          return (
            <div key={row.cat}
              role="button" tabIndex={0}
              onMouseEnter={() => setHover(row.cat)}
              onMouseLeave={() => setHover(null)}
              className="flex items-center gap-2 rounded-md px-1.5 py-1 -mx-1.5 transition-colors"
              style={{ background: hover === row.cat ? 'var(--t-row-hover)' : 'transparent', cursor: 'default' }}>
              <span className="inline-block w-2.5 h-2.5 rounded-full shrink-0" style={{ background: row.color }} aria-hidden="true" />
              <span className="mt-body-sm truncate flex-1 min-w-0" style={{ color: 'var(--t-muted)' }}>
                {row.label}
              </span>
              <span className="mt-mono-sm text-[10px] shrink-0" style={{ color: 'var(--t-faint)' }}>{pct}%</span>
              <span className="mt-mono-sm text-[11px] shrink-0 w-11 text-right" style={{ color: 'var(--t-faint)' }}>{fmtDur(row.seconds)}</span>
            </div>
          )
        })}
      </div>
    </div>
  )
}
