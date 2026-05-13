// meridian — normalises screenpipe activity into structured app sessions
'use client'

import { useEffect, useState } from 'react'
import { CATS, CatDot, SectionHead, Card } from '@/components/atoms'
import type { WeekResponse } from '@/app/api/week/route'

export default function WeekView() {
  const [data, setData] = useState<WeekResponse | null>(null)

  useEffect(() => {
    fetch('/api/week').then(r => r.json()).then(setData).catch(() => {})
  }, [])

  if (!data) {
    return (
      <div className="space-y-10">
        <header className="rise">
          <p className="text-[11px] uppercase tracking-[0.2em]" style={{ color: 'var(--ink-3)' }}>Last 7 days</p>
          <h1 className="font-serif text-[56px] leading-[1] tracking-tight mt-1" style={{ color: 'var(--ink)' }}>Your week in shape</h1>
        </header>
        <p className="text-[13px]" style={{ color: 'var(--ink-3)' }}>Loading…</p>
      </div>
    )
  }

  const maxTotal = Math.max(...data.days.map(d => d.total_s), 1)

  // aggregate category totals across the week
  const catSums: Record<string, number> = {}
  data.days.forEach(d => {
    Object.entries(d.cats).forEach(([k, v]) => {
      catSums[k] = (catSums[k] ?? 0) + v
    })
  })

  const totalH = data.total_s / 3600

  return (
    <div className="space-y-10">
      <header className="rise flex items-end justify-between">
        <div>
          <p className="text-[11px] uppercase tracking-[0.2em]" style={{ color: 'var(--ink-3)' }}>Last 7 days</p>
          <h1 className="font-serif text-[56px] leading-[1] tracking-tight mt-1" style={{ color: 'var(--ink)' }}>
            Your week in shape
          </h1>
        </div>
        <div className="text-right">
          <p className="font-mono tnum text-[32px] leading-none" style={{ color: 'var(--ink)' }}>{totalH.toFixed(1)}h</p>
          <p className="text-[11px] mt-1.5" style={{ color: 'var(--ink-3)' }}>focus across the week</p>
        </div>
      </header>

      {/* Stacked bars */}
      <div className="grid grid-cols-7 gap-3">
        {data.days.map(d => (
          <DayBar key={d.date} day={d} maxTotal_s={maxTotal} />
        ))}
      </div>

      {/* Category breakdown */}
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
          <p className="font-serif italic text-[24px]" style={{ color: 'var(--ink-2)' }}>Nothing recorded yet.</p>
          <p className="text-[12px] mt-2" style={{ color: 'var(--ink-3)' }}>Start meridian to begin tracking activity.</p>
        </div>
      )}
    </div>
  )
}

function DayBar({ day, maxTotal_s }: { day: WeekResponse['days'][number]; maxTotal_s: number }) {
  const cats = Object.entries(day.cats).sort((a, b) => b[1] - a[1])
  const sum = cats.reduce((a, [, v]) => a + v, 0) || 0.0001
  const heightPct = (day.total_s / maxTotal_s) * 100

  return (
    <div className="flex flex-col items-stretch">
      <div className="h-[220px] flex items-end">
        <div className="w-full rounded-md overflow-hidden flex flex-col-reverse"
          style={{
            height: `${heightPct}%`,
            minHeight: day.total_s > 0 ? 4 : 2,
            background: day.total_s === 0 ? 'var(--rule)' : 'transparent',
          }}>
          {cats.map(([cat, v]) => (
            <div key={cat} className={`cat-${cat}`}
              style={{ height: `${(v / sum) * 100}%` }}
              title={`${CATS[cat]?.label ?? cat} · ${v.toFixed(1)}h`} />
          ))}
        </div>
      </div>
      <div className="mt-3 text-center">
        <p className="text-[11px]" style={{ color: day.isToday ? 'var(--accent)' : 'var(--ink-3)' }}>
          {day.day}{day.isToday ? ' · today' : ''}
        </p>
        <p className="font-mono tnum text-[12px] mt-0.5" style={{ color: 'var(--ink)' }}>
          {day.total_s > 0 ? `${(day.total_s / 3600).toFixed(1)}h` : '—'}
        </p>
        <p className="font-mono tnum text-[10px] mt-0.5" style={{ color: 'var(--ink-4)' }}>{day.date}</p>
      </div>
    </div>
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
