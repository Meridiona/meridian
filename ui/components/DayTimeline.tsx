//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

import { useMemo, useState } from 'react'
import { fmtDur } from '@/components/atoms'
import type { TodayResponse } from '@/lib/api-types'

// The hero visual of the Today view. Two aligned tracks tell the whole story at
// a glance, with overlap shown rather than summed:
//   • Presence — solid where you were active, faint where you were idle/away.
//   • Agent    — a band beneath presence, solid where Claude/Codex was engaged.
// Where the agent band sits under an IDLE stretch of the presence track, that is
// autonomous work (agent running while you were away) — visible by alignment, no
// number needed. Hover any block for its exact span.

type Band = { startMs: number; endMs: number; label: string; kind: 'active' | 'idle' | 'agent' | 'paused' }

const COLOR = {
  active: 'var(--ink)',
  idle: 'var(--rule-2)',
  agent: '#3B6FE0', // matches the `coding` category hue used across the app
  paused: '#F59E0B', // amber — distinct from idle so users can see deliberate pauses
}

const ms = (iso: string) => new Date(iso).getTime()

export default function DayTimeline({ data }: { data: TodayResponse }) {
  const [hover, setHover] = useState<Band | null>(null)

  const model = useMemo(() => {
    const active: Band[] = data.presence_segments.map(s => ({
      startMs: ms(s.started_at), endMs: ms(s.ended_at), kind: 'active' as const, label: 'Active',
    }))
    const idle: Band[] = data.gaps
      .filter(g => g.kind === 'user_idle')
      .map(g => ({ startMs: ms(g.started_at), endMs: ms(g.ended_at), kind: 'idle' as const, label: 'Idle' }))
    const paused: Band[] = data.gaps
      .filter(g => g.kind === 'tracking_paused' || g.kind === 'schedule_paused')
      .map(g => ({
        startMs: ms(g.started_at), endMs: ms(g.ended_at), kind: 'paused' as const,
        label: g.kind === 'schedule_paused' ? 'Outside work hours' : 'Paused',
      }))
    const agent: Band[] = data.agent_segments.map(s => ({
      startMs: ms(s.started_at), endMs: ms(s.ended_at), kind: 'agent' as const, label: 'Claude / Codex',
    }))

    const all = [...active, ...idle, ...paused, ...agent].filter(b => Number.isFinite(b.startMs) && b.endMs > b.startMs)
    if (all.length === 0) return null

    // Window: floor to the hour before the first event, ceil to the hour after
    // the last — so bands never touch the edges and hour ticks line up.
    const HOUR = 3_600_000
    const lo = Math.floor(Math.min(...all.map(b => b.startMs)) / HOUR) * HOUR
    const hi = Math.ceil(Math.max(...all.map(b => b.endMs)) / HOUR) * HOUR
    const span = Math.max(HOUR, hi - lo)

    const ticks: { left: number; label: string }[] = []
    for (let t = lo; t <= hi; t += HOUR) {
      ticks.push({ left: ((t - lo) / span) * 100, label: new Date(t).getHours().toString().padStart(2, '0') })
    }
    const pos = (b: Band) => ({
      left: ((b.startMs - lo) / span) * 100,
      width: Math.max(0.4, ((b.endMs - b.startMs) / span) * 100),
    })
    return { active, idle, paused, agent, ticks, pos }
  }, [data])

  if (!model) return null

  const tip = () => {
    if (!hover) return null
    const fmt = (t: number) => new Date(t).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
    return (
      <div className="text-[11px] font-mono tnum px-2 py-1 rounded-md inline-flex gap-2 items-center"
        style={{ background: 'var(--ink)', color: 'var(--paper)' }}>
        <span style={{ opacity: 0.7 }}>{hover.label}</span>
        <span>{fmt(hover.startMs)}–{fmt(hover.endMs)}</span>
        <span style={{ opacity: 0.7 }}>· {fmtDur(Math.round((hover.endMs - hover.startMs) / 1000))}</span>
      </div>
    )
  }

  const block = (b: Band, top: number, height: number, color: string, rounded = false) => {
    const { left, width } = model.pos(b)
    return (
      <div
        key={`${b.kind}-${b.startMs}`}
        onMouseEnter={() => setHover(b)}
        onMouseLeave={() => setHover(h => (h === b ? null : h))}
        className="absolute cursor-pointer transition-opacity"
        style={{
          left: `${left}%`, width: `${width}%`, top, height,
          background: color, borderRadius: rounded ? 3 : 2,
          opacity: hover && hover !== b ? 0.4 : 1,
        }}
      />
    )
  }

  return (
    <div className="select-none">
      {/* tooltip rail keeps height stable whether or not something is hovered */}
      <div className="h-6 mb-1 flex items-end">{tip()}</div>

      <div className="relative" style={{ height: 54 }}>
        {/* hour gridlines */}
        {model.ticks.map((t, i) => (
          <div key={`g${i}`} className="absolute top-0" style={{ left: `${t.left}%`, height: 40, width: 1, background: 'var(--rule)' }} />
        ))}
        {/* presence track: idle baseline, paused (amber) above idle, active on top */}
        {model.idle.map(b => block(b, 6, 18, COLOR.idle))}
        {model.paused.map(b => block(b, 6, 18, COLOR.paused))}
        {model.active.map(b => block(b, 6, 18, COLOR.active))}
        {/* agent overlay track, aligned beneath presence */}
        {model.agent.map(b => block(b, 27, 11, COLOR.agent, true))}
        {/* hour labels */}
        {model.ticks.map((t, i) => (
          <div key={`l${i}`} className="absolute font-mono text-[9px]"
            style={{ left: `${t.left}%`, top: 42, color: 'var(--ink-4)', transform: 'translateX(-50%)' }}>
            {t.label}
          </div>
        ))}
      </div>

      {/* legend */}
      <div className="mt-3 flex flex-wrap items-center gap-x-4 gap-y-1 text-[11px]" style={{ color: 'var(--ink-3)' }}>
        <span className="inline-flex items-center gap-1.5"><i style={{ width: 10, height: 10, background: COLOR.active, borderRadius: 2, display: 'inline-block' }} /> Active</span>
        <span className="inline-flex items-center gap-1.5"><i style={{ width: 10, height: 10, background: COLOR.idle, borderRadius: 2, display: 'inline-block' }} /> Idle</span>
        <span className="inline-flex items-center gap-1.5"><i style={{ width: 10, height: 10, background: COLOR.paused, borderRadius: 2, display: 'inline-block' }} /> Paused</span>
        <span className="inline-flex items-center gap-1.5"><i style={{ width: 10, height: 6, background: COLOR.agent, borderRadius: 2, display: 'inline-block' }} /> Claude / Codex</span>
        <span style={{ color: 'var(--ink-4)' }}>· agent under an idle stretch = autonomous</span>
      </div>
    </div>
  )
}
