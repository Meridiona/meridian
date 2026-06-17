//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

import { useState, useMemo, useEffect, Fragment } from 'react'
import { useRouter } from 'next/navigation'
import {
  fmtDur, fmtDurDecimal, fmtClock,
  CATS, AppGlyph, CatDot, LiveDot, ProviderGlyph, PROVIDER_META,
  TaskKey, ConfidenceRing, SegBar, SectionHead, Card, useTick,
} from '@/components/atoms'
import TaskBadge from '@/components/TaskBadge'
import ShapeOfDay from '@/components/ShapeOfDay'
import DayTimeline from '@/components/DayTimeline'
import TodayMetrics from '@/components/TodayMetrics'
import type { TodayResponse, AgentSummary } from '@/app/api/today/route'

interface BucketSession {
  id: number | string
  app: string
  started_at: string
  dur: number
  cat: string
  titles: string[]
  task_key: string | null
  session_type: string | null
  link_method: string | null
  link_confidence: number | null
  routing: string | null
  summary: string | null
}

interface Bucket {
  key: string
  title: string
  sessions: BucketSession[]
  total_s: number
  autonomous_s: number  // agent time that ran while you were away (0 for non-task buckets)
  cats: Set<string>
  day_total_s: number
  isOverhead: boolean
  isUntracked: boolean
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
              <p className="type-active" style={{ color: 'var(--ink)' }}>
                You&apos;re on
              </p>
              {taskKey && (
                <TaskKey keyId={taskKey} big />
              )}
              <span className="type-active truncate" style={{ color: 'var(--ink)' }}>
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

// ── At-a-glance insight (Layer 1) ────────────────────────────────────────────
// One pre-computed takeaway, in the Oura "tell me the meaning, not the data"
// spirit: lead with active focus + how much was AI-assisted, and only mention
// autonomous agent work or a fragmented day when they're actually notable.
function buildInsight(data: TodayResponse): string | null {
  const { focus_s, supervised_s, autonomous_s, switch_count, active } = data
  if (focus_s < 300 && !active) return null

  const aiPct = focus_s > 0 ? Math.round((supervised_s / focus_s) * 100) : 0
  let lead = `${fmtDur(focus_s)} focused`
  if (aiPct >= 5) lead += `, ${aiPct}% of it alongside Claude`

  const extra: string[] = []
  if (autonomous_s >= 600) extra.push(`plus ${fmtDur(autonomous_s)} of autonomous agent work while you were away`)
  const focusMin = focus_s / 60
  if (focusMin > 60 && switch_count > focusMin / 4) extra.push(`a fragmented day — ${switch_count} context switches`)

  return extra.length ? `${lead} — ${extra.join('; ')}.` : `${lead}.`
}

// ── Per-task summary chunks ──────────────────────────────────────────────────
// Sessions on a task (foreground + coding-agent) are clustered into rolling
// windows of at most CHUNK_MAX_SPAN_S, broken early on a CHUNK_MAX_GAP_S lull —
// one digest line per stretch of work rather than one per session, so an
// expanded task reads as a short narrative instead of a wall of text.
const CHUNK_MAX_SPAN_S = 2 * 3600
const CHUNK_MAX_GAP_S = 30 * 60

interface SummaryChunk {
  start: string
  end: string
  text: string
}

/** First sentence of a session summary, capped — summaries run 10-40 sentences. */
function summaryLead(text: string): string {
  const t = text.trim()
  const m = t.match(/^[^.!?]*[.!?]/)
  const s = (m ? m[0] : t).trim()
  return s.length > 240 ? s.slice(0, 237).trimEnd() + '…' : s
}

function buildSummaryChunks(sessions: BucketSession[], agentSummaries: AgentSummary[]): SummaryChunk[] {
  const entries = [
    ...sessions.filter(s => s.summary).map(s => ({ started_at: s.started_at, dur: s.dur, summary: s.summary! })),
    ...agentSummaries,
  ].sort((a, b) => new Date(a.started_at).getTime() - new Date(b.started_at).getTime())

  const chunks: Array<{ startMs: number; endMs: number; leads: string[] }> = []
  for (const e of entries) {
    const startMs = new Date(e.started_at).getTime()
    const endMs = startMs + Math.max(0, e.dur) * 1000
    const lead = summaryLead(e.summary)
    const cur = chunks[chunks.length - 1]
    if (cur && startMs - cur.startMs <= CHUNK_MAX_SPAN_S * 1000 && startMs - cur.endMs <= CHUNK_MAX_GAP_S * 1000) {
      cur.endMs = Math.max(cur.endMs, endMs)
      if (lead && !cur.leads.includes(lead)) cur.leads.push(lead)
    } else {
      chunks.push({ startMs, endMs, leads: lead ? [lead] : [] })
    }
  }

  // Newest stretch first, matching the session list below it.
  return chunks
    .filter(c => c.leads.length > 0)
    .map(c => ({
      start: new Date(c.startMs).toISOString(),
      end: new Date(c.endMs).toISOString(),
      text: c.leads.join(' '),
    }))
    .reverse()
}

// ── Task bucket row ──────────────────────────────────────────────────────────
function BucketRow({ bucket, agentSummaries = [] }: { bucket: Bucket; agentSummaries?: AgentSummary[] }) {
  const [open, setOpen] = useState(false)
  const segs = useMemo(() => {
    const byCat: Record<string, number> = {}
    bucket.sessions.forEach(s => { byCat[s.cat] = (byCat[s.cat] ?? 0) + s.dur })
    return Object.entries(byCat).map(([cat, value]) => ({ cat, value }))
  }, [bucket.sessions])

  // Latest work on top — both the digest chunks and the raw session list.
  const recentFirst = useMemo(
    () => [...bucket.sessions].sort((a, b) => new Date(b.started_at).getTime() - new Date(a.started_at).getTime()),
    [bucket.sessions],
  )
  const chunks = useMemo(
    () => buildSummaryChunks(bucket.sessions, agentSummaries),
    [bucket.sessions, agentSummaries],
  )

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
        <span className="font-mono tnum text-[12px] w-[92px] shrink-0" style={{ color: 'var(--ink-2)' }}>
          {!bucket.isOverhead && !bucket.isUntracked && !bucket.isQueue
            ? <TaskKey keyId={bucket.key} />
            : <span style={{ color: 'var(--ink-3)' }}>
                {bucket.isOverhead ? 'overhead' : bucket.isUntracked ? 'untracked' : 'needs review'}
              </span>}
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
          <p className="text-[11px] mt-1.5" style={{ color: 'var(--ink-3)' }}>
            {pct}% of day
            {bucket.autonomous_s >= 60 && (
              <span style={{ color: 'var(--live)' }}
                title="Of this total, the agent ran on its own while you were away from the keyboard — the part that adds time beyond your own.">
                {' · +'}{fmtDur(bucket.autonomous_s)} agent while away
              </span>
            )}
          </p>
        </div>
      </button>

      {open && (
        <div className="px-5 pb-4 pt-1 rule-t" style={{ borderTopColor: 'var(--rule)' }}>
          {chunks.length > 0 && (
            <div className="py-3 space-y-2.5">
              <p className="text-[10px] uppercase tracking-[0.16em]" style={{ color: 'var(--ink-3)' }}>Summary</p>
              {chunks.map(c => (
                <div key={c.start} className="grid grid-cols-[auto_1fr] gap-3">
                  <span className="font-mono tnum text-[11px] pt-px whitespace-nowrap" style={{ color: 'var(--ink-3)' }}>
                    {fmtClock(c.start)}–{fmtClock(c.end)}
                  </span>
                  <p className="text-[12px] leading-relaxed" style={{ color: 'var(--ink-2)', textWrap: 'pretty' } as React.CSSProperties}>
                    {c.text}
                  </p>
                </div>
              ))}
              <p className="text-[10px] uppercase tracking-[0.16em] pt-1" style={{ color: 'var(--ink-3)' }}>Sessions</p>
            </div>
          )}
          <div className="grid grid-cols-1 gap-px" style={{ background: 'var(--rule)' }}>
            {recentFirst.map(s => (
              <div key={s.id} className="grid grid-cols-[auto_1fr_auto_auto] items-center gap-4 py-2.5 px-3"
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
                <TaskBadge
                  taskKey={s.task_key}
                  sessionType={s.session_type}
                  routing={s.routing}
                  confidence={s.link_confidence}
                  method={s.link_method}
                  size="xs"
                />
                <span className="font-mono tnum text-[12px]" style={{ color: 'var(--ink-2)' }}>{fmtDur(s.dur)}</span>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  )
}

// ── Integration group header ─────────────────────────────────────────────────
function GroupHead({ provider, label, total_s }: { provider?: string; label?: string; total_s: number }) {
  const meta = provider ? PROVIDER_META[provider] : undefined
  return (
    <div className="flex items-center gap-2 px-5 py-2" style={{ background: 'var(--surface-2)' }}>
      {provider && meta && <ProviderGlyph provider={provider} size={16} />}
      <span className="text-[10px] uppercase tracking-[0.15em]" style={{ color: 'var(--ink-3)' }}>
        {label ?? meta?.label ?? provider}
      </span>
      <span className="ml-auto font-mono tnum text-[10px]" style={{ color: 'var(--ink-4)' }}>{fmtDur(total_s)}</span>
    </div>
  )
}

// ── Story so far ─────────────────────────────────────────────────────────────
function buildStory(data: TodayResponse): string | null {
  const { sessions, active } = data
  if (sessions.length === 0 && !active) return null

  // Durations come from task_totals (your time + autonomous agent) so the
  // narrative matches the "By task" buckets exactly; categories come from the
  // foreground sessions for the verb choice.
  const taskMap = new Map<string, { dur: number; auto: number; cats: string[] }>()
  sessions.forEach(s => {
    const key = s.task_key
    if (!key) return
    if (!taskMap.has(key)) taskMap.set(key, { dur: data.task_totals[key] ?? 0, auto: data.task_autonomous_s[key] ?? 0, cats: [] })
    const e = taskMap.get(key)!
    if (!e.cats.includes(s.cat)) e.cats.push(s.cat)
  })

  const untracked_filter = (s: typeof sessions[number]) => !s.task_key && s.routing !== 'queue'
  const overhead = sessions
    .filter(s => untracked_filter(s) && s.session_type === 'overhead')
    .reduce((a, s) => a + s.dur, 0)
  const untracked = sessions
    .filter(s => untracked_filter(s) && s.session_type !== 'overhead')
    .reduce((a, s) => a + s.dur, 0)

  const tasks = Array.from(taskMap.entries())
    .sort((a, b) => b[1].dur - a[1].dur)
    .slice(0, 3)

  if (tasks.length === 0 && overhead + untracked < 600 && !active) return null

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
    // Call out autonomous agent help on the lead task — the standout insight.
    const aside = i === 0 && info.auto >= 600 ? ` (${fmtDur(info.auto)} of it the agent, solo)` : ''
    if (i === 0) clauses.push(`${verbFor(info.cats, info.dur)} ${key} for ${dur}${aside}`)
    else clauses.push(`${dur} on ${key}`)
  })

  if (overhead > 900) clauses.push(`${fmtDur(overhead)} of overhead`)
  if (untracked > 900) clauses.push(`${fmtDur(untracked)} untracked`)

  if (active) {
    const catLabel = CATS[active.cat]?.label?.toLowerCase() ?? active.cat
    clauses.push(tasks.length > 0
      ? `currently in ${catLabel}`
      : `started ${catLabel} at ${fmtClock(active.started_at)}`)
  }

  return clauses.length > 0 ? clauses.join(', ') + '.' : null
}

// ── Queue preview row ─────────────────────────────────────────────────────────
function QueuePreviewRow({ session }: { session: { id: number | string; app: string; started_at: string; dur: number; titles: string[]; candidates?: string[] } }) {
  return (
    <div className="grid grid-cols-[auto_1fr_auto_auto] items-center gap-5 px-5 py-4 rule-t"
         style={{ borderTopColor: 'var(--rule)' }}>
      <AppGlyph app={session.app} size={24} />
      <div className="min-w-0">
        <p className="text-[13px] truncate" style={{ color: 'var(--ink)' }}>{session.titles[0] || session.app}</p>
        <p className="text-[11px] mt-1" style={{ color: 'var(--ink-3)' }}>
          <span className="font-mono tnum">{fmtClock(session.started_at)}</span>
          {session.candidates && session.candidates.length > 0 && (
            <span className="ml-2 text-[11px]" style={{ color: 'var(--accent)' }}>· needs review</span>
          )}
        </p>
      </div>
      <div className="flex items-center gap-1.5">
        {session.candidates?.map(c => <TaskKey key={c} keyId={c} />)}
      </div>
      <span className="font-mono tnum text-[12px]" style={{ color: 'var(--ink-2)' }}>{fmtDur(session.dur)}</span>
    </div>
  )
}

// ── Today page ───────────────────────────────────────────────────────────────
export default function TodayView() {
  const router = useRouter()
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
          <h1 className="type-hero" style={{ color: 'var(--ink)' }}>Today</h1>
        </header>
        <p className="text-[13px]" style={{ color: 'var(--ink-3)' }}>Loading…</p>
      </div>
    )
  }

  // Total time in coding category: foreground coding sessions + active if coding
  // + autonomous agent time (Claude Code / Codex ran while you were away).
  const coding_s = data.sessions.filter(s => s.cat === 'coding').reduce((a, s) => a + s.dur, 0)
    + (data.active?.cat === 'coding' ? data.active.elapsed_s : 0)
    + (data.autonomous_s ?? 0)

  // Build buckets. The "% of day" denominator is total ENGAGED time (your
  // foreground presence + autonomous agent time), so a task carrying agent work
  // never exceeds 100%. Falls back to focus_s on older API responses.
  const total_s = data.engaged_s || data.focus_s
  const bucketMap = new Map<string, Bucket>()

  function pushToBucket(key: string, session: BucketSession) {
    if (!bucketMap.has(key)) {
      bucketMap.set(key, { key, title: '', sessions: [], total_s: 0, autonomous_s: 0, cats: new Set(), day_total_s: total_s, isOverhead: false, isUntracked: false, isQueue: false })
    }
    const b = bucketMap.get(key)!
    b.sessions.push(session)
    b.total_s += session.dur
    b.cats.add(session.cat)
  }

  data.sessions.forEach(s => {
    const bs: BucketSession = { id: s.id, app: s.app, started_at: s.started_at, dur: s.dur, cat: s.cat, titles: s.titles, task_key: s.task_key, session_type: s.session_type, link_method: s.link_method, link_confidence: s.link_confidence, routing: s.routing, summary: s.summary }
    if (s.task_key) pushToBucket(s.task_key, bs)
    else if (s.routing === 'queue') pushToBucket('_queue', bs)
    // session_type ('task' | 'overhead' | 'unknown') is the classifier's own
    // call: 'overhead' is work-adjacent (comms, planning, meetings) with no
    // ticket; everything else untracked (unknown / null / personal / idle).
    else if (s.session_type === 'overhead') pushToBucket('_overhead', bs)
    else pushToBucket('_untracked', bs)
  })

  if (data.active) {
    const ab: BucketSession = { id: 'active', app: data.active.app, started_at: data.active.started_at, dur: data.active.elapsed_s, cat: data.active.cat, titles: data.active.titles, task_key: null, session_type: null, link_method: null, link_confidence: null, routing: null, summary: null }
    pushToBucket('_active', ab)
  }

  const buckets = Array.from(bucketMap.values()).map(b => {
    // For a real task bucket, the headline time is the UNION of total time on
    // the task (foreground + capped agent overlay) computed server-side — the
    // same figure the Tasks page shows, so the two views always agree. Pushing
    // foreground session durs above only filled `b.total_s` as a fallback.
    const isTask = !b.key.startsWith('_')
    const total = isTask ? (data.task_totals[b.key] ?? b.total_s) : b.total_s
    const autonomous_s = isTask ? (data.task_autonomous_s[b.key] ?? 0) : 0
    return {
      ...b,
      total_s: total,
      autonomous_s,
      isOverhead: b.key === '_overhead',
      isUntracked: b.key === '_untracked',
      isQueue: b.key === '_queue',
      title: b.key === '_overhead'  ? 'Overhead — comms, mail, planning'
           : b.key === '_untracked' ? 'Untracked — personal, idle, unclassified'
           : b.key === '_queue'     ? 'Sessions waiting to be assigned'
           : b.key === '_active'    ? 'Current session'
           // The human-readable ticket title; the key itself stays visible as
           // the small mono badge in the row's left column. Falls back to the
           // key when the ticket was pruned from pm_tasks (closed + re-synced).
           : data.task_meta?.[b.key]?.title ?? b.key,
      day_total_s: total_s,
    }
  }).sort((a, b) => {
    const ord = (k: string) =>
      k === '_queue' ? 10 : k === '_untracked' ? 9 : k === '_overhead' ? 8 : k === '_active' ? -1 : 0
    return (ord(a.key) - ord(b.key)) || (b.total_s - a.total_s)
  })

  // ── Group task buckets by integration ──────────────────────────────────────
  // The active block leads, then one group per tracker (largest first), then
  // the off-ticket buckets (overhead / untracked / queue) at the bottom.
  const activeBucket = buckets.find(b => b.key === '_active')
  const taskBuckets = buckets.filter(b => !b.key.startsWith('_'))
  const offTicketBuckets = buckets.filter(b => b.key.startsWith('_') && b.key !== '_active')

  const byProvider = new Map<string, Bucket[]>()
  taskBuckets.forEach(b => {
    const provider = data.task_meta?.[b.key]?.provider ?? 'other'
    if (!byProvider.has(provider)) byProvider.set(provider, [])
    byProvider.get(provider)!.push(b)
  })
  const providerGroups = Array.from(byProvider.entries())
    .map(([provider, group]) => ({ provider, group, total_s: group.reduce((a, g) => a + g.total_s, 0) }))
    .sort((a, b) => b.total_s - a.total_s)
  const offTicketTotal = offTicketBuckets.reduce((a, b) => a + b.total_s, 0)

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
        <h1 className="type-hero" style={{ color: 'var(--ink)' }}>Today</h1>
      </header>

      {/* Layer 1 — the glance: one insight + the day timeline (overlap shown, not summed) */}
      {(() => {
        const insight = buildInsight(data)
        const story = buildStory(data)
        if (!insight && !(data.sessions.length > 0 || data.active)) return null
        return (
          <section className="rise space-y-5">
            {insight && (
              <div>
                <p className="text-[11px] uppercase tracking-[0.18em] mb-3" style={{ color: 'var(--ink-3)' }}>Today at a glance</p>
                <p className="type-stat" style={{ color: 'var(--ink)', textWrap: 'pretty' } as React.CSSProperties}>{insight}</p>
              </div>
            )}
            {(data.sessions.length > 0 || data.active) && (
              <Card className="p-6"><DayTimeline data={data} /></Card>
            )}
            {story && (
              <p className="text-[14px] leading-relaxed" style={{ color: 'var(--ink-2)', textWrap: 'pretty' } as React.CSSProperties}>{story}</p>
            )}
          </section>
        )
      })()}

      {data.active && <ActiveSessionCard active={data.active} taskKey={data.sessions.filter(s => s.task_key).at(-1)?.task_key} />}

      {/* Layer 2 + 3 — zoom row with details-on-demand */}
      <TodayMetrics
        focus_s={data.focus_s}
        idle_s={data.idle_s}
        agent_s={data.agent_s}
        supervised_s={data.supervised_s}
        autonomous_s={data.autonomous_s}
        coding_s={coding_s}
        switch_count={data.switch_count}
      />

      {buckets.length > 0 && (
        <section>
          <SectionHead kicker="By task" title="Where your time went"
            right={<button onClick={() => router.push('/tasks')} className="text-[12px]" style={{ color: 'var(--ink-3)' }}>Open Tasks →</button>}
          />
          <div className="space-y-px rule rounded-xl overflow-hidden border" style={{ borderColor: 'var(--rule)' }}>
            {activeBucket && <BucketRow bucket={activeBucket} />}
            {providerGroups.map(g => (
              <Fragment key={g.provider}>
                <GroupHead provider={g.provider} label={g.provider === 'other' ? 'Other tasks' : undefined} total_s={g.total_s} />
                {g.group.map(b => (
                  <BucketRow key={b.key} bucket={b} agentSummaries={data.task_agent_summaries?.[b.key] ?? []} />
                ))}
              </Fragment>
            ))}
            {offTicketBuckets.length > 0 && (
              <Fragment>
                {(activeBucket || providerGroups.length > 0) && (
                  <GroupHead label="Off-ticket" total_s={offTicketTotal} />
                )}
                {offTicketBuckets.map(b => <BucketRow key={b.key} bucket={b} />)}
              </Fragment>
            )}
          </div>
        </section>
      )}

      {(data.sessions.length > 0 || data.active) && (
        <section>
          <SectionHead kicker="Shape of the day" title="Activity patterns &amp; insights" />
          <ShapeOfDay data={data} />
        </section>
      )}

      {queueCount > 0 && (
        <section>
          <SectionHead
            kicker="Needs review"
            title={<>Unassigned sessions <span className="ml-2 font-mono tnum text-[15px]" style={{ color: 'var(--accent)' }}>{queueCount}</span></>}
            right={<button onClick={() => router.push('/queue')} className="text-[12px]" style={{ color: 'var(--ink-3)' }}>Review queue →</button>}
          />
          <Card>
            {data.sessions
              .filter(s => s.routing === 'queue')
              .slice(0, 3)
              .map(s => (
                <QueuePreviewRow key={s.id} session={{ ...s, candidates: s.task_key ? [s.task_key] : [] }} />
              ))}
          </Card>
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
