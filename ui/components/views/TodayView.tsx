// meridian — normalises screenpipe activity into structured app sessions
'use client'

import { useState, useMemo, useEffect } from 'react'
import {
  fmtDur, fmtDurDecimal, fmtClock,
  CATS, AppGlyph, CatDot, CatLabel, LiveDot,
  TaskKey, ConfidenceRing, SegBar, SectionHead, Card, useTick,
} from '@/components/atoms'
import type { TodayResponse } from '@/app/api/today/route'

interface BucketSession {
  id: number | string
  app: string
  started_at: string
  dur: number
  cat: string
  titles: string[]
  task_key: string | null
}

interface Bucket {
  key: string
  title: string
  sessions: BucketSession[]
  total_s: number
  cats: Set<string>
  day_total_s: number
  isOverhead: boolean
  isQueue: boolean
}

// ── Active session card ──────────────────────────────────────────────────────
function ActiveSessionCard({ active, taskKey }: { active: NonNullable<TodayResponse['active']>; taskKey?: string | null }) {
  const tick = useTick(1)
  const elapsed = active.elapsed_s + tick
  const frames = Math.max(1, Math.floor(elapsed / 30))

  return (
    <Card className="overflow-hidden rise" style={{ borderColor: 'var(--rule-2)' }}>
      <div className="h-[3px] w-full" style={{ background: 'var(--rule)' }}>
        <div className="h-full blink" style={{ width: '32%', background: 'var(--live)' }} />
      </div>
      <div className="p-5">
        {/* Header row */}
        <div className="flex items-center gap-3 mb-4">
          <LiveDot size={8} />
          <span className="text-[10px] uppercase tracking-[0.2em]" style={{ color: 'var(--ink-3)' }}>
            Live · captured {frames} frame{frames === 1 ? '' : 's'} ago
          </span>
          <span className="ml-auto text-[11px]" style={{ color: 'var(--ink-3)' }}>
            <span className="font-mono tnum">{fmtClock(active.started_at)}</span> → now
          </span>
        </div>

        <div className="flex items-start gap-5">
          <div className="flex-1 min-w-0">
            {/* "You're on" headline */}
            <div className="flex items-baseline gap-3 flex-wrap">
              <p className="text-[28px] leading-none italic" style={{ color: 'var(--ink)', fontFamily: "'Instrument Serif', Georgia, serif" }}>
                You&apos;re on
              </p>
              {taskKey && (
                <TaskKey keyId={taskKey} big />
              )}
              <span className="text-[28px] leading-none italic truncate" style={{ color: 'var(--ink)', fontFamily: "'Instrument Serif', Georgia, serif" }}>
                {active.cat !== 'idle_personal' ? CATS[active.cat]?.label?.toLowerCase() ?? active.cat : 'an uncategorized session'}
              </span>
            </div>

            {/* Context row: app, cat */}
            <div className="mt-4 flex flex-wrap items-center gap-x-5 gap-y-2 text-[13px]" style={{ color: 'var(--ink-2)' }}>
              <span className="inline-flex items-center gap-2">
                <AppGlyph app={active.app} size={20} />
                <span style={{ color: 'var(--ink)' }}>{active.app}</span>
              </span>
              <span className="inline-flex items-center gap-1.5">
                <CatDot cat={active.cat} />
                <span>{CATS[active.cat]?.label ?? active.cat}</span>
              </span>
            </div>

            {/* Explanation */}
            {active.explain && (
              <p className="mt-4 text-[12px] leading-relaxed" style={{ color: 'var(--ink-3)' }}>
                <span style={{ color: 'var(--ink-2)' }}>Why this task</span>
                <span className="mx-2" style={{ color: 'var(--ink-4)' }}>·</span>
                {active.explain}
              </p>
            )}
          </div>

          {/* Elapsed + confidence */}
          <div className="text-right shrink-0 pl-4 rule-l" style={{ borderLeftColor: 'var(--rule)' }}>
            <p className="text-[10px] uppercase tracking-[0.2em]" style={{ color: 'var(--ink-3)' }}>elapsed</p>
            <p className="font-mono tnum text-[40px] leading-none mt-2" style={{ color: 'var(--ink)' }}>
              {fmtDurDecimal(elapsed)}
            </p>
            <div className="mt-3 flex items-center justify-end gap-1.5">
              <ConfidenceRing value={active.confidence} size={14} />
              <span className="text-[11px] font-mono tnum" style={{ color: 'var(--ink-3)' }}>
                {Math.round(active.confidence * 100)}%
              </span>
            </div>
          </div>
        </div>

        {/* Window title pills */}
        {active.titles.length > 0 && (
          <div className="mt-5 flex flex-wrap gap-1.5">
            {active.titles.slice(0, 5).map(w => (
              <span key={w} className="text-[11px] font-mono px-2 py-1 rounded-md tnum"
                style={{ background: 'var(--surface-2)', color: 'var(--ink-2)', border: '1px solid var(--rule)' }}>
                {w}
              </span>
            ))}
          </div>
        )}

        {/* Jira log action row */}
        <div className="mt-5 pt-4 rule-t flex items-center justify-between gap-3" style={{ borderTopColor: 'var(--rule)' }}>
          <p className="text-[12px]" style={{ color: 'var(--ink-2)' }}>
            Log this session to Jira when it closes?
          </p>
          <div className="flex items-center gap-2">
            <button className="text-[12px] px-3 py-1.5 rounded-md" style={{ color: 'var(--ink-3)' }}>
              Skip
            </button>
            <button className="text-[12px] px-3 py-1.5 rounded-md font-medium"
              style={{ color: 'var(--paper)', background: 'var(--ink)' }}>
              {taskKey ? `Log to ${taskKey} →` : 'Assign task →'}
            </button>
          </div>
        </div>
      </div>
    </Card>
  )
}

// ── Totals strip ─────────────────────────────────────────────────────────────
function TotalsStrip({ focus_s, idle_s, session_count }: { focus_s: number; idle_s: number; session_count: number }) {
  const items = [
    { k: 'Focus',    v: fmtDur(focus_s),    note: 'active' },
    { k: 'Idle',     v: fmtDur(idle_s),     note: 'away from keyboard' },
    { k: 'Sessions', v: String(session_count), note: 'captured today' },
  ]
  return (
    <div className="grid grid-cols-3 rule-t rule-b" style={{ borderColor: 'var(--rule)' }}>
      {items.map((it, i) => (
        <div key={it.k} className={`py-4 px-5 ${i > 0 ? 'rule-l' : ''}`} style={{ borderColor: 'var(--rule)' }}>
          <p className="text-[10px] uppercase tracking-[0.16em] mb-2" style={{ color: 'var(--ink-3)' }}>{it.k}</p>
          <p className="font-mono tnum text-[20px] leading-none" style={{ color: 'var(--ink)' }}>{it.v}</p>
          <p className="text-[11px] mt-1.5" style={{ color: 'var(--ink-3)' }}>{it.note}</p>
        </div>
      ))}
    </div>
  )
}

// ── Task bucket row ──────────────────────────────────────────────────────────
function BucketRow({ bucket }: { bucket: Bucket }) {
  const [open, setOpen] = useState(false)
  const segs = useMemo(() => {
    const byCat: Record<string, number> = {}
    bucket.sessions.forEach(s => { byCat[s.cat] = (byCat[s.cat] ?? 0) + s.dur })
    return Object.entries(byCat).map(([cat, value]) => ({ cat, value }))
  }, [bucket.sessions])

  const pct = bucket.day_total_s > 0
    ? ((bucket.total_s / bucket.day_total_s) * 100).toFixed(0)
    : '0'

  return (
    <div style={{ background: 'var(--surface)' }}>
      <button
        onClick={() => setOpen(o => !o)}
        className="w-full text-left grid grid-cols-[auto_1fr_200px_auto] items-center gap-5 px-5 py-4 transition-colors"
        style={{ background: open ? 'var(--surface-2)' : 'var(--surface)' }}
      >
        <span className="font-mono tnum text-[12px] w-[72px]" style={{ color: 'var(--ink-2)' }}>
          {!bucket.isOverhead && !bucket.isQueue
            ? <TaskKey keyId={bucket.key} />
            : <span style={{ color: 'var(--ink-3)' }}>{bucket.isOverhead ? 'overhead' : 'needs review'}</span>}
        </span>
        <div className="min-w-0">
          <p className="text-[14px] truncate" style={{ color: 'var(--ink)' }}>{bucket.title}</p>
          <p className="text-[11px] mt-1.5" style={{ color: 'var(--ink-3)' }}>
            {bucket.sessions.length} session{bucket.sessions.length === 1 ? '' : 's'}
            {segs.length > 0 && ` · ${segs.slice(0, 3).map(s => CATS[s.cat]?.short ?? s.cat).join(' + ')}`}
          </p>
        </div>
        <div className="hidden md:block">
          <SegBar segments={segs.length ? segs : [{ value: 1, color: 'var(--rule-2)' }]} height={3} />
        </div>
        <div className="text-right">
          <p className="font-mono tnum text-[18px] leading-none" style={{ color: 'var(--ink)' }}>{fmtDur(bucket.total_s)}</p>
          <p className="text-[11px] mt-1.5" style={{ color: 'var(--ink-3)' }}>{pct}% of day</p>
        </div>
      </button>

      {open && (
        <div className="px-5 pb-4 pt-1 rule-t" style={{ borderTopColor: 'var(--rule)' }}>
          <div className="grid grid-cols-1 gap-px" style={{ background: 'var(--rule)' }}>
            {bucket.sessions.map(s => (
              <div key={s.id} className="grid grid-cols-[auto_1fr_auto] items-center gap-4 py-2.5 px-3"
                style={{ background: 'var(--surface)' }}>
                <AppGlyph app={s.app} size={20} />
                <div className="min-w-0">
                  <p className="text-[13px] truncate" style={{ color: 'var(--ink)' }}>{s.titles[0] || '—'}</p>
                  <div className="flex items-center gap-2 mt-0.5">
                    <span className="font-mono tnum text-[11px]" style={{ color: 'var(--ink-3)' }}>{fmtClock(s.started_at)}</span>
                    <CatDot cat={s.cat} />
                    <span className="text-[11px]" style={{ color: 'var(--ink-3)' }}>{CATS[s.cat]?.short ?? s.cat}</span>
                  </div>
                </div>
                <span className="font-mono tnum text-[12px]" style={{ color: 'var(--ink-2)' }}>{fmtDur(s.dur)}</span>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  )
}

// ── Story so far ─────────────────────────────────────────────────────────────
function buildStory(data: TodayResponse): string | null {
  const { sessions, active } = data
  if (sessions.length === 0 && !active) return null

  const taskMap = new Map<string, { dur: number; cats: string[] }>()
  sessions.forEach(s => {
    const key = s.task_key
    if (!key) return
    if (!taskMap.has(key)) taskMap.set(key, { dur: 0, cats: [] })
    const e = taskMap.get(key)!
    e.dur += s.dur
    if (!e.cats.includes(s.cat)) e.cats.push(s.cat)
  })

  const overhead = sessions
    .filter(s => !s.task_key && s.routing !== 'queue')
    .reduce((a, s) => a + s.dur, 0)

  const tasks = Array.from(taskMap.entries())
    .sort((a, b) => b[1].dur - a[1].dur)
    .slice(0, 3)

  if (tasks.length === 0 && overhead < 600 && !active) return null

  const verbFor = (cats: string[], dur: number): string => {
    const c = cats[0]
    if (c === 'coding')            return dur > 3600 ? 'Deep on'    : 'Coding on'
    if (c === 'code_review')       return 'Reviewing'
    if (c === 'design')            return 'Designing'
    if (c === 'documentation')     return 'Docs for'
    if (c === 'planning')          return 'Planning'
    if (c === 'meeting')           return 'Meetings around'
    if (c === 'communication')     return 'Comms on'
    if (c === 'research')          return 'Researching'
    if (c === 'deployment_devops') return 'Deploying'
    return 'Working on'
  }

  const clauses: string[] = []
  tasks.forEach(([key, info], i) => {
    const dur = fmtDur(info.dur)
    if (i === 0) clauses.push(`${verbFor(info.cats, info.dur)} ${key} for ${dur}`)
    else clauses.push(`${dur} on ${key}`)
  })

  if (overhead > 900) clauses.push(`${fmtDur(overhead)} of overhead`)

  if (active) {
    const catLabel = CATS[active.cat]?.label?.toLowerCase() ?? active.cat
    clauses.push(tasks.length > 0
      ? `currently in ${catLabel}`
      : `started ${catLabel} at ${fmtClock(active.started_at)}`)
  }

  return clauses.length > 0 ? clauses.join(', ') + '.' : null
}

// ── Day timeline ─────────────────────────────────────────────────────────────
function DayTimeline({ data }: { data: TodayResponse }) {
  const [hover, setHover] = useState<null | {
    app: string; cat: string; started_at: string; dur: number; titles: string[]; live?: boolean
  }>(null)

  const DAY_START = 7, DAY_END = 19
  const span = DAY_END - DAY_START
  const pct = (h: number) => Math.max(0, Math.min(100, ((h - DAY_START) / span) * 100))
  const toH = (iso: string) => new Date(iso).getHours() + new Date(iso).getMinutes() / 60

  const hours = [7, 9, 11, 13, 15, 17, 19]

  return (
    <div className="select-none">
      <div className="relative h-4 mb-2">
        {hours.map(h => (
          <span key={h} className="absolute font-mono tnum text-[10px] -translate-x-1/2"
            style={{ left: pct(h) + '%', color: 'var(--ink-4)' }}>
            {((h + 11) % 12) + 1}{h >= 12 ? 'p' : 'a'}
          </span>
        ))}
      </div>

      <div className="relative h-10 rounded-lg overflow-hidden" style={{ background: 'var(--rule)' }}>
        {Array.from({ length: span + 1 }, (_, i) => i + DAY_START).map(h => (
          <div key={h} className="absolute top-0 bottom-0 w-px"
            style={{ left: pct(h) + '%', background: 'var(--paper)', opacity: .6 }} />
        ))}

        {data.gaps.map(g => {
          const l = pct(toH(g.started_at)), w = Math.max(0.4, pct(toH(g.ended_at)) - l)
          return (
            <div key={g.id} className="absolute top-0 bottom-0"
              style={{ left: l + '%', width: w + '%',
                background: 'repeating-linear-gradient(135deg, var(--rule) 0 4px, transparent 4px 8px)' }} />
          )
        })}

        {data.sessions.map(s => {
          const sh = toH(s.started_at)
          const eh = sh + s.dur / 3600
          const l = pct(sh), w = Math.max(0.4, pct(eh) - l)
          return (
            <div key={s.id}
              onMouseEnter={() => setHover({ app: s.app, cat: s.cat, started_at: s.started_at, dur: s.dur, titles: s.titles })}
              onMouseLeave={() => setHover(null)}
              className="absolute top-0 bottom-0 cursor-pointer transition-[filter] hover:brightness-110"
              style={{ left: l + '%', width: w + '%' }}>
              <div className={`w-full h-full cat-${s.cat}`} />
            </div>
          )
        })}

        {data.active && (() => {
          const sh = toH(data.active.started_at)
          const eh = sh + data.active.elapsed_s / 3600
          const l = pct(sh), w = Math.max(0.4, pct(eh) - l)
          return (
            <div
              onMouseEnter={() => setHover({ app: data.active!.app, cat: data.active!.cat, started_at: data.active!.started_at, dur: data.active!.elapsed_s, titles: data.active!.titles, live: true })}
              onMouseLeave={() => setHover(null)}
              className={`absolute top-0 bottom-0 cursor-pointer blink`}
              style={{ left: l + '%', width: w + '%' }}>
              <div className={`w-full h-full cat-${data.active.cat}`} />
            </div>
          )
        })()}
      </div>

      <div className="mt-4 flex flex-wrap items-center gap-x-4 gap-y-2">
        {Array.from(new Set(data.sessions.map(s => s.cat).concat(data.active ? [data.active.cat] : []))).map(c => (
          <CatLabel key={c} cat={c} />
        ))}
      </div>

      <div className="mt-4 min-h-[44px]">
        {hover ? (
          <div className="flex items-center gap-4 text-[12px] rise">
            <AppGlyph app={hover.app} size={20} />
            <span style={{ color: 'var(--ink)' }}>{hover.titles[0] || hover.app}</span>
            <CatLabel cat={hover.cat} />
            <span className="font-mono tnum" style={{ color: 'var(--ink-3)' }}>
              {fmtClock(hover.started_at)} · {fmtDur(hover.dur)}
            </span>
            {hover.live && <span className="text-[11px] inline-flex items-center gap-1.5" style={{ color: 'var(--live)' }}>
              <LiveDot size={6} /> live
            </span>}
          </div>
        ) : (
          <p className="text-[11px]" style={{ color: 'var(--ink-4)' }}>Hover the timeline to inspect a session.</p>
        )}
      </div>
    </div>
  )
}

// ── Today page ───────────────────────────────────────────────────────────────
export default function TodayView({ onNavigate }: { onNavigate?: (v: string, key?: string) => void }) {
  const [data, setData] = useState<TodayResponse | null>(null)

  useEffect(() => {
    fetch('/api/today').then(r => r.json()).then(setData).catch(() => {})
    const id = setInterval(() => fetch('/api/today').then(r => r.json()).then(setData).catch(() => {}), 30_000)
    return () => clearInterval(id)
  }, [])

  if (!data) {
    return (
      <div className="space-y-12">
        <header className="rise">
          <h1 className="leading-[0.95] tracking-tight" style={{ color: 'var(--ink)', fontFamily: "'Instrument Serif', Georgia, serif" }}>Today</h1>
        </header>
        <p className="text-[13px]" style={{ color: 'var(--ink-3)' }}>Loading…</p>
      </div>
    )
  }

  // Build buckets
  const total_s = data.focus_s
  const bucketMap = new Map<string, Bucket>()

  function pushToBucket(key: string, session: BucketSession) {
    if (!bucketMap.has(key)) {
      bucketMap.set(key, { key, title: '', sessions: [], total_s: 0, cats: new Set(), day_total_s: total_s, isOverhead: false, isQueue: false })
    }
    const b = bucketMap.get(key)!
    b.sessions.push(session)
    b.total_s += session.dur
    b.cats.add(session.cat)
  }

  data.sessions.forEach(s => {
    const bs: BucketSession = { id: s.id, app: s.app, started_at: s.started_at, dur: s.dur, cat: s.cat, titles: s.titles, task_key: s.task_key }
    if (s.task_key) pushToBucket(s.task_key, bs)
    else if (s.routing === 'queue') pushToBucket('_queue', bs)
    else pushToBucket('_overhead', bs)
  })

  if (data.active) {
    const ab: BucketSession = { id: 'active', app: data.active.app, started_at: data.active.started_at, dur: data.active.elapsed_s, cat: data.active.cat, titles: data.active.titles, task_key: null }
    pushToBucket('_active', ab)
  }

  const buckets = Array.from(bucketMap.values()).map(b => ({
    ...b,
    isOverhead: b.key === '_overhead',
    isQueue: b.key === '_queue',
    title: b.key === '_overhead' ? 'Overhead — comms, mail, planning'
         : b.key === '_queue'    ? 'Sessions waiting to be assigned'
         : b.key === '_active'   ? 'Current session'
         : b.key,
    day_total_s: total_s,
  })).sort((a, b) => {
    const ord = (k: string) => k === '_queue' ? 9 : k === '_overhead' ? 8 : k === '_active' ? -1 : 0
    return (ord(a.key) - ord(b.key)) || (b.total_s - a.total_s)
  })

  const queueCount = data.sessions.filter(s => s.routing === 'queue').length

  const dateLabel = new Date().toLocaleDateString('en-US', { weekday: 'long', month: 'long', day: 'numeric' })

  return (
    <div className="space-y-12">
      <header className="rise">
        <div className="flex items-baseline justify-between mb-1">
          <p className="text-[11px] uppercase tracking-[0.2em]" style={{ color: 'var(--ink-3)' }}>{dateLabel}</p>
          {data.active && (
            <p className="text-[11px]" style={{ color: 'var(--ink-3)' }}>
              Last capture <span className="font-mono tnum">{fmtClock(data.active.started_at)}</span>
            </p>
          )}
        </div>
        <h1 className="text-[88px] leading-[0.95] tracking-tight" style={{ color: 'var(--ink)', fontFamily: "'Instrument Serif', Georgia, serif" }}>Today</h1>
      </header>

      {(() => {
        const story = buildStory(data)
        return story ? (
          <section className="rise">
            <p className="text-[11px] uppercase tracking-[0.18em] mb-3" style={{ color: 'var(--ink-3)' }}>The story so far</p>
            <p className="text-[26px] leading-[1.2]" style={{ color: 'var(--ink)', textWrap: 'pretty', fontFamily: "'Instrument Serif', Georgia, serif" } as React.CSSProperties}>{story}</p>
          </section>
        ) : null
      })()}

      {data.active && <ActiveSessionCard active={data.active} taskKey={data.sessions.filter(s => s.task_key).at(-1)?.task_key} />}

      <TotalsStrip focus_s={data.focus_s} idle_s={data.idle_s} session_count={data.session_count} />

      {buckets.length > 0 && (
        <section>
          <SectionHead kicker="By task" title="Where your time went"
            right={<button onClick={() => onNavigate?.('tasks')} className="text-[12px]" style={{ color: 'var(--ink-3)' }}>Open Tasks →</button>}
          />
          <div className="space-y-px rule rounded-xl overflow-hidden border" style={{ borderColor: 'var(--rule)' }}>
            {buckets.map(b => <BucketRow key={b.key} bucket={b} />)}
          </div>
        </section>
      )}

      {(data.sessions.length > 0 || data.active) && (
        <section>
          <SectionHead kicker="Timeline" title="The shape of the day" />
          <Card className="p-6">
            <DayTimeline data={data} />
          </Card>
        </section>
      )}

      {queueCount > 0 && (
        <section>
          <SectionHead
            kicker="Queue"
            title={<>Needs review <span className="ml-2 font-mono tnum text-[15px]" style={{ color: 'var(--accent)' }}>{queueCount}</span></>}
            right={<button onClick={() => onNavigate?.('queue')} className="text-[12px]" style={{ color: 'var(--ink-3)' }}>Open Queue →</button>}
          />
        </section>
      )}

      {data.session_count === 0 && !data.active && (
        <div className="rounded-xl border py-16 text-center" style={{ borderColor: 'var(--rule)', background: 'var(--surface)' }}>
          <p className="text-[13px]" style={{ color: 'var(--ink-3)' }}>No activity captured today</p>
          <p className="text-[11px] mt-1" style={{ color: 'var(--ink-4)' }}>Start meridian to begin tracking</p>
        </div>
      )}
    </div>
  )
}
