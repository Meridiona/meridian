//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

import { useState } from 'react'
import { fmtDur } from '@/components/atoms'

// Layer 2 + 3 of the Today view (Shneiderman: zoom, then details-on-demand).
// A calm row of three headline numbers — Focus, AI-assisted, Switches — each a
// button that expands one detail panel below. The fuller breakdown (Active vs
// Idle, Supervised vs Autonomous) stays hidden until asked for, so the default
// view never clogs.

interface Props {
  focus_s: number
  idle_s: number
  agent_s: number
  supervised_s: number
  autonomous_s: number
  coding_s: number
  switch_count: number
}

type Key = 'focus' | 'coding' | 'switches'

const pct = (part: number, whole: number) => (whole > 0 ? Math.round((part / whole) * 100) : 0)

function DetailRow({ label, value, hint }: { label: string; value: string; hint?: string }) {
  return (
    <div className="flex items-baseline justify-between gap-4 py-2">
      <div className="min-w-0">
        <span className="text-[13px]" style={{ color: 'var(--ink)' }}>{label}</span>
        {hint && <span className="text-[11px] ml-2" style={{ color: 'var(--ink-3)' }}>{hint}</span>}
      </div>
      <span className="font-mono tnum text-[14px] whitespace-nowrap" style={{ color: 'var(--ink)' }}>{value}</span>
    </div>
  )
}

export default function TodayMetrics(props: Props) {
  const { focus_s, idle_s, agent_s, supervised_s, autonomous_s, coding_s, switch_count } = props
  const [open, setOpen] = useState<Key | null>(null)

  const tiles: { key: Key; label: string; value: string; note: string }[] = [
    { key: 'focus', label: 'Focus', value: fmtDur(focus_s), note: 'active' },
    { key: 'coding', label: 'Coding', value: fmtDur(coding_s), note: 'time coding' },
    { key: 'switches', label: 'Switches', value: String(switch_count), note: 'context switches' },
  ]
  const showAgentLine = autonomous_s >= 60

  const detail = (key: Key) => {
    switch (key) {
      case 'focus':
        return (
          <>
            <DetailRow label="Active" value={fmtDur(focus_s)} hint="you, at the keyboard" />
            <DetailRow label="Idle / away" value={fmtDur(idle_s)} hint="no input detected" />
          </>
        )
      case 'coding':
        return (
          <>
            <DetailRow label="Your coding" value={fmtDur(coding_s - autonomous_s)} hint="at the keyboard" />
            <DetailRow label="AI coding (solo)" value={fmtDur(autonomous_s)} hint="Claude / Codex ran while you were away" />
            <DetailRow label="Total" value={fmtDur(coding_s)} hint="your coding + AI agent" />
          </>
        )
      case 'switches':
        return (
          <>
            <DetailRow label="Context switches" value={String(switch_count)} hint="foreground app changes over 15s" />
            <p className="text-[12px] leading-relaxed pt-1" style={{ color: 'var(--ink-3)' }}>
              Lower is deeper. Brief sub-15s window flicker is filtered out so this
              reflects real context shifts, not capture noise.
            </p>
          </>
        )
    }
  }

  return (
    <div className="rule-t rule-b" style={{ borderColor: 'var(--rule)' }}>
      <div className="grid grid-cols-3">
        {tiles.map((t, i) => {
          const isOpen = open === t.key
          return (
            <button
              key={t.key}
              onClick={() => setOpen(isOpen ? null : t.key)}
              className={`text-left py-4 px-5 transition-colors ${i > 0 ? 'rule-l' : ''}`}
              style={{ borderColor: 'var(--rule)', background: isOpen ? 'var(--tint)' : 'transparent' }}
              aria-expanded={isOpen}
            >
              <p className="text-[10px] uppercase tracking-[0.16em] mb-2 flex items-center gap-1.5" style={{ color: 'var(--ink-3)' }}>
                {t.label}
                <span className="text-[9px]" style={{ color: 'var(--ink-4)', transform: isOpen ? 'rotate(180deg)' : 'none', transition: 'transform 0.15s' }}>▾</span>
              </p>
              <p className="font-mono tnum text-[20px] leading-none" style={{ color: 'var(--ink)' }}>{t.value}</p>
              {t.key === 'coding' && showAgentLine
                ? <p className="text-[11px] mt-1.5" style={{ color: 'var(--live)' }}>incl. {fmtDur(autonomous_s)} autonomous AI agent</p>
                : <p className="text-[11px] mt-1.5" style={{ color: 'var(--ink-3)' }}>{t.note}</p>
              }
            </button>
          )
        })}
      </div>

      {open && (
        <div className="px-5 py-3 rule-t" style={{ borderTopColor: 'var(--rule)', background: 'var(--surface)' }}>
          <div className="max-w-md">{detail(open)}</div>
        </div>
      )}
    </div>
  )
}
