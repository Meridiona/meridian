//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

import { useEffect, useState, useMemo } from 'react'
import { CATS, CatDot, SectionHead, Card } from '@/components/atoms'
import type { WeekResponse } from '@/lib/api-types'
import { load } from '@/lib/bridge'

export default function WeekView() {
  const [data, setData] = useState<WeekResponse | null>(null)

  useEffect(() => {
    // get_week (Rust) in the Tauri window, /api/week in a browser — same shape.
    load<WeekResponse>('/api/week', 'get_week').then(setData).catch(() => {})
  }, [])

  if (!data) {
    return (
      <div className="space-y-10">
        <header className="rise">
          <p className="text-[11px] uppercase tracking-[0.2em]" style={{ color: 'var(--ink-3)' }}>Last 7 days</p>
          <h1 className="type-title mt-1" style={{ color: 'var(--ink)' }}>Your week in shape</h1>
        </header>
        <p className="text-[13px]" style={{ color: 'var(--ink-3)' }}>Loading…</p>
      </div>
    )
  }

  const catSums: Record<string, number> = {}
  data.days.forEach(d => {
    Object.entries(d.cats).forEach(([k, v]) => { catSums[k] = (catSums[k] ?? 0) + v })
  })

  const totalH = data.total_s / 3600
  const maxTotal = Math.max(...data.days.map(d => d.total_s / 3600), 0.1)

  // Compute insights from data
  const bestDay = data.days.reduce((a, d) => d.total_s > a.total_s ? d : a, data.days[0])
  const topCat = Object.entries(catSums).sort((a, b) => b[1] - a[1])[0]
  const todayData = data.days.find(d => d.isToday)

  return (
    <div className="space-y-10">
      <header className="rise flex items-end justify-between">
        <div>
          <p className="text-[11px] uppercase tracking-[0.2em]" style={{ color: 'var(--ink-3)' }}>Last 7 days</p>
          <h1 className="type-title mt-1" style={{ color: 'var(--ink)' }}>
            Your week in shape
          </h1>
        </div>
        <div className="text-right">
          <p className="font-mono tnum text-[32px] leading-none" style={{ color: 'var(--ink)' }}>{totalH.toFixed(1)}h</p>
          <p className="text-[11px] mt-1.5" style={{ color: 'var(--ink-3)' }}>focus across the week</p>
        </div>
      </header>

      <Card className="p-6 flex justify-center">
        <WeekLineChart days={data.days} maxTotal={maxTotal} />
      </Card>

      <div className="grid grid-cols-3 gap-6">
        {bestDay && bestDay.total_s > 0 && (
          <Insight
            kicker="Deep work"
            headline={`${bestDay.day} won`}
            body={`${(bestDay.total_s / 3600).toFixed(1)} hours of focus. ${bestDay.isToday ? "That's today." : 'Best day of the week.'}`}
          />
        )}
        {topCat && (
          <Insight
            kicker="Top category"
            headline={CATS[topCat[0]]?.label ?? topCat[0]}
            body={`${topCat[1].toFixed(1)} hours this week. ${((topCat[1] / totalH) * 100).toFixed(0)}% of all focus time.`}
          />
        )}
        <Insight
          kicker="Week total"
          headline={`${totalH.toFixed(1)}h tracked`}
          body={todayData && todayData.total_s > 0
            ? `Today contributed ${(todayData.total_s / 3600).toFixed(1)}h so far.`
            : 'Captured silently in the background.'}
        />
      </div>

      {Object.keys(catSums).length > 0 && (
        <section>
          <SectionHead kicker="By category" title="Where the hours actually went" />
          <Card className="p-6">
            <CategoryBars catSums={catSums} totalH={totalH} />
          </Card>
        </section>
      )}

      {data.total_s === 0 && (
        <div className="py-16 text-center rounded-xl border" style={{ borderColor: 'var(--rule)', background: 'var(--surface)' }}>
          <p className="type-empty" style={{ color: 'var(--ink-2)' }}>Nothing recorded yet.</p>
          <p className="text-[12px] mt-2" style={{ color: 'var(--ink-3)' }}>Start meridian to begin tracking activity.</p>
        </div>
      )}
    </div>
  )
}

function WeekLineChart({ days, maxTotal }: { days: WeekResponse['days']; maxTotal: number }) {
  const [hoveredIndex, setHoveredIndex] = useState<number | null>(null)
  const width = 700, height = 140
  const padding = { top: 15, right: 15, bottom: 30, left: 40 }
  const chartW = width - padding.left - padding.right
  const chartH = height - padding.top - padding.bottom

  const points = useMemo(() => days.map((d, i) => ({
    x: padding.left + (i / Math.max(days.length - 1, 1)) * chartW,
    y: padding.top + chartH - ((d.total_s / 3600) / maxTotal) * chartH,
    day: d,
    index: i,
  })), [days, maxTotal, chartW, chartH])

  const linePath = points.map((p, i) => `${i === 0 ? 'M' : 'L'} ${p.x} ${p.y}`).join(' ')
  const areaPath =
    `M ${padding.left} ${padding.top + chartH} ` +
    points.map(p => `L ${p.x} ${p.y}`).join(' ') +
    ` L ${padding.left + chartW} ${padding.top + chartH} Z`

  return (
    <svg width={width} height={height} viewBox={`0 0 ${width} ${height}`}>
      {[0, 0.5, 1].map(ratio => {
        const y = padding.top + chartH - ratio * chartH
        return <line key={ratio} x1={padding.left} y1={y} x2={padding.left + chartW} y2={y}
          stroke="var(--rule)" strokeWidth="1" />
      })}

      <path d={areaPath} fill="var(--accent)"
        opacity={hoveredIndex !== null ? 0.15 : 0.1}
        style={{ transition: 'opacity 0.15s ease' }} />

      <path d={linePath} fill="none" stroke="var(--accent)"
        strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" />

      {points.map((p, i) => {
        const isHovered = hoveredIndex === i
        const isToday = p.day.isToday
        return (
          <g key={i}>
            <circle cx={p.x} cy={p.y} r={isHovered ? 6 : isToday ? 5 : 4}
              fill={isToday || isHovered ? 'var(--accent)' : 'var(--surface)'}
              stroke="var(--accent)" strokeWidth="2"
              style={{ cursor: 'pointer', transition: 'all 0.15s ease' }} />
            <circle cx={p.x} cy={p.y} r={20} fill="transparent" style={{ cursor: 'pointer' }}
              onMouseEnter={() => setHoveredIndex(i)}
              onMouseLeave={() => setHoveredIndex(null)} />
          </g>
        )
      })}

      {points.map((p, i) => (
        <text key={i} x={p.x} y={padding.top + chartH + 16} textAnchor="middle"
          fontSize="10" fill={p.day.isToday ? 'var(--accent)' : 'var(--ink-3)'}
          fontFamily="var(--font-mono)">
          {p.day.day}
        </text>
      ))}

      {[0, maxTotal / 2, maxTotal].map((val, i) => {
        const y = padding.top + chartH - (val / maxTotal) * chartH
        return (
          <text key={i} x={padding.left - 8} y={y + 3} textAnchor="end"
            fontSize="9" fill="var(--ink-3)" fontFamily="var(--font-mono)">
            {val.toFixed(1)}h
          </text>
        )
      })}
    </svg>
  )
}

function Insight({ kicker, headline, body }: { kicker: string; headline: string; body: string }) {
  return (
    <Card className="p-5">
      <p className="text-[10px] uppercase tracking-[0.18em] mb-2" style={{ color: 'var(--ink-3)' }}>{kicker}</p>
      <p className="type-callout" style={{ color: 'var(--ink)' }}>{headline}</p>
      <p className="text-[12px] mt-2 leading-relaxed" style={{ color: 'var(--ink-2)' }}>{body}</p>
    </Card>
  )
}

function CategoryBars({ catSums, totalH }: { catSums: Record<string, number>; totalH: number }) {
  const items = Object.entries(catSums).sort((a, b) => b[1] - a[1])
  return (
    <div className="space-y-3">
      {items.map(([cat, h]) => (
        <div key={cat} className="grid grid-cols-[120px_1fr_64px] items-center gap-4">
          <span className="inline-flex items-center gap-2 text-[12px]" style={{ color: 'var(--ink-2)' }}>
            <CatDot cat={cat} /> {CATS[cat]?.label ?? cat}
          </span>
          <div className="h-2 rounded-full overflow-hidden" style={{ background: 'var(--rule)' }}>
            <div className={`h-full cat-${cat}`} style={{ width: `${(h / totalH) * 100}%` }} />
          </div>
          <span className="font-mono tnum text-[12px] text-right" style={{ color: 'var(--ink)' }}>{h.toFixed(1)}h</span>
        </div>
      ))}
    </div>
  )
}
