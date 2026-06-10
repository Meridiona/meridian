// meridian — normalises screenpipe activity into structured app sessions
'use client'

import { useState, useMemo } from 'react'
import { fmtDur, CATS, CatDot, Card } from '@/components/atoms'

const CAT_COLORS: Record<string, string> = {
  coding:            '#3B6FE0',
  code_review:       '#7C3AED',
  meeting:           '#D97706',
  communication:     '#059669',
  design:            '#DB2777',
  documentation:     '#0891B2',
  planning:          '#C4822A',
  deployment_devops: '#DC2626',
  research:          '#4F46E5',
  idle_personal:     '#78716C',
}
import type { TodayResponse } from '@/app/api/today/route'

export default function ShapeOfDay({ data }: { data: TodayResponse }) {
  const toH = (iso: string) => new Date(iso).getHours() + new Date(iso).getMinutes() / 60

  const catData = useMemo(() => {
    const byCat: Record<string, number> = {}
    data.sessions.forEach(s => { byCat[s.cat] = (byCat[s.cat] || 0) + s.dur })
    if (data.active) byCat[data.active.cat] = (byCat[data.active.cat] || 0) + data.active.elapsed_s
    const total = Object.values(byCat).reduce((sum, v) => sum + v, 0) || 1
    return Object.entries(byCat).map(([cat, seconds]) => ({
      cat, seconds,
      percentage: (seconds / total) * 100,
      label: CATS[cat]?.label || cat,
    })).sort((a, b) => b.seconds - a.seconds)
  }, [data])

  const longestFocusBlock = useMemo(() => {
    const activities = [
      ...data.sessions.map(s => ({ kind: 'session' as const, startH: toH(s.started_at), dur: s.dur })),
      ...(data.active ? [{ kind: 'session' as const, startH: toH(data.active.started_at), dur: data.active.elapsed_s }] : []),
      ...data.gaps.map(g => ({ kind: 'gap' as const, startH: toH(g.started_at), dur: g.dur })),
    ].sort((a, b) => a.startH - b.startH)
    let maxBlock = 0, currentBlock = 0
    activities.forEach(act => {
      if (act.kind === 'gap' && act.dur > 300) { maxBlock = Math.max(maxBlock, currentBlock); currentBlock = 0 }
      else if (act.kind === 'session') currentBlock += act.dur
    })
    return Math.max(maxBlock, currentBlock)
  }, [data])

  const slices = useMemo(() => {
    let currentAngle = 0
    const cx = 100, cy = 100, outerR = 85, innerR = 58
    return catData.map(item => {
      const startAngle = currentAngle
      const angleSize = (item.percentage / 100) * 360
      const endAngle = startAngle + angleSize
      const sr = (startAngle - 90) * Math.PI / 180
      const er = (endAngle - 90) * Math.PI / 180
      const largeArc = angleSize > 180 ? 1 : 0
      const path = [
        `M ${cx + outerR * Math.cos(sr)} ${cy + outerR * Math.sin(sr)}`,
        `A ${outerR} ${outerR} 0 ${largeArc} 1 ${cx + outerR * Math.cos(er)} ${cy + outerR * Math.sin(er)}`,
        `L ${cx + innerR * Math.cos(er)} ${cy + innerR * Math.sin(er)}`,
        `A ${innerR} ${innerR} 0 ${largeArc} 0 ${cx + innerR * Math.cos(sr)} ${cy + innerR * Math.sin(sr)}`,
        'Z',
      ].join(' ')
      currentAngle = endAngle
      return { ...item, path }
    })
  }, [catData])

  const [hoveredCat, setHoveredCat] = useState<string | null>(null)

  return (
    <Card className="p-6">
      <div className="flex items-center gap-8">
        <div className="flex-1 flex justify-center">
          <div className="relative" style={{ width: 200, height: 200 }}>
            <svg width="200" height="200" viewBox="0 0 200 200">
              {slices.map(slice => (
                <path
                  key={slice.cat}
                  d={slice.path}
                  className={`cat-${slice.cat}`}
                  opacity={hoveredCat === slice.cat ? 1 : hoveredCat === null ? 0.95 : 0.35}
                  style={{
                    cursor: 'pointer',
                    transition: 'opacity 0.2s ease, transform 0.15s ease',
                    transformOrigin: '100px 100px',
                    transform: hoveredCat === slice.cat ? 'scale(1.05)' : 'scale(1)',
                  }}
                  onMouseEnter={() => setHoveredCat(slice.cat)}
                  onMouseLeave={() => setHoveredCat(null)}
                />
              ))}
            </svg>
            <div className="absolute inset-0 flex flex-col items-center justify-center pointer-events-none">
              <p className="font-mono tnum text-[28px] leading-none" style={{ color: 'var(--ink)' }}>
                {fmtDur(data.focus_s)}
              </p>
              <p className="text-[10px] uppercase tracking-wide mt-1" style={{ color: 'var(--ink-3)' }}>active</p>
            </div>
          </div>
        </div>

        <div className="flex-1 space-y-4">
          <div className="grid grid-cols-2 gap-3 pb-4 rule-b" style={{ borderBottomColor: 'var(--rule)' }}>
            <div>
              <p className="text-[10px] uppercase tracking-wide mb-1" style={{ color: 'var(--ink-3)' }}>Longest block</p>
              <p className="font-mono tnum text-[20px] leading-none" style={{ color: 'var(--success)' }}>{fmtDur(longestFocusBlock)}</p>
            </div>
            <div>
              <p className="text-[10px] uppercase tracking-wide mb-1" style={{ color: 'var(--ink-3)' }}>Idle time</p>
              <p className="font-mono tnum text-[20px] leading-none" style={{ color: 'var(--ink-3)' }}>{fmtDur(data.idle_s)}</p>
            </div>
          </div>
          <div className="space-y-0.5">
            {catData.map(item => {
              const isHov = hoveredCat === item.cat
              return (
                <div
                  key={item.cat}
                  className="flex items-center justify-between gap-3 py-1.5 px-2 rounded-md transition-all cursor-pointer"
                  style={{
                    background: isHov ? 'var(--tint)' : 'transparent',
                    borderLeft: `3px solid ${isHov ? (CAT_COLORS[item.cat] ?? 'var(--accent)') : 'transparent'}`,
                    opacity: isHov ? 1 : hoveredCat === null ? 1 : 0.45,
                    paddingLeft: isHov ? '6px' : '8px',
                  }}
                  onMouseEnter={() => setHoveredCat(item.cat)}
                  onMouseLeave={() => setHoveredCat(null)}
                >
                  <div className="flex items-center gap-2 flex-1 min-w-0">
                    <CatDot cat={item.cat} size={9} />
                    <span className="text-[12px] truncate font-medium" style={{ color: isHov ? 'var(--ink)' : 'var(--ink-2)' }}>
                      {item.label}
                    </span>
                  </div>
                  <span className="text-[12px] font-mono tnum font-semibold" style={{ color: isHov ? 'var(--ink)' : 'var(--ink-3)' }}>
                    {isHov ? fmtDur(item.seconds) : `${item.percentage.toFixed(0)}%`}
                  </span>
                </div>
              )
            })}
          </div>
        </div>
      </div>
    </Card>
  )
}
