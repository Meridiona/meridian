// meridian — AI activity intelligence by Meridiona

import getDb from '@/lib/db'
import { localDayBounds, todayString } from '@/lib/date-utils'
import { formatDateLabel } from '@/lib/format'
import ActiveSessionCard from '@/components/ActiveSessionCard'
import DayTimeline from '@/components/DayTimeline'
import StatsRow from '@/components/StatsRow'
import SessionCard from '@/components/SessionCard'
import RefreshTrigger from '@/components/RefreshTrigger'
import CategoryBreakdown from '@/components/CategoryBreakdown'
import type {
  ActiveSessionRow, StatsResponse, TimelineResponse, SessionRow, GapRow
} from '@/lib/types'

export const revalidate = 30

function parseSession(r: Record<string, unknown>): SessionRow {
  return {
    id: r.id as number,
    app_name: r.app_name as string,
    started_at: r.started_at as string,
    ended_at: r.ended_at as string,
    duration_s: r.duration_s as number,
    window_titles: JSON.parse((r.window_titles as string) || '[]'),
    audio_snippets: r.audio_snippets ? JSON.parse(r.audio_snippets as string) : null,
    signals: r.signals ? JSON.parse(r.signals as string) : null,
    frame_count: r.frame_count as number,
    etl_run_id: r.etl_run_id as number,
    category: (r.category as string) || 'idle_personal',
    confidence: (r.confidence as number) || 0,
  }
}

function getActiveSession(): ActiveSessionRow | null {
  try {
    const db = getDb()
    const row = db.prepare(`
      SELECT app_name, started_at, last_seen_at,
             window_titles, audio_snippets, frame_count,
             category, confidence
      FROM active_session WHERE id = 1
    `).get() as Record<string, unknown> | undefined
    if (!row) return null
    return {
      app_name: row.app_name as string,
      started_at: row.started_at as string,
      last_seen_at: row.last_seen_at as string,
      window_titles: JSON.parse((row.window_titles as string) || '[]'),
      audio_snippets: row.audio_snippets ? JSON.parse(row.audio_snippets as string) : null,
      signals: null,
      frame_count: row.frame_count as number,
      elapsed_s: Math.floor((Date.now() - new Date(row.started_at as string).getTime()) / 1000),
      category: (row.category as string) || 'idle_personal',
      confidence: (row.confidence as number) || 0,
    }
  } catch { return null }
}

function getStats(date: string): StatsResponse {
  const empty: StatsResponse = { date, focus_s: 0, user_idle_s: 0, away_s: 0, session_count: 0, top_apps: [], category_breakdown: [] }
  try {
    const db = getDb()
    const { start, end } = localDayBounds(date)
    const t = db.prepare(`
      SELECT
        SUM(duration_s) AS focus_s,
        COUNT(*) AS session_count
      FROM app_sessions WHERE started_at >= ? AND started_at < ?
    `).get(start, end) as { focus_s: number | null; session_count: number }
    const topApps = db.prepare(`
      SELECT app_name, SUM(duration_s) AS duration_s, COUNT(*) AS session_count
      FROM app_sessions WHERE started_at >= ? AND started_at < ?
      GROUP BY app_name ORDER BY duration_s DESC LIMIT 8
    `).all(start, end) as StatsResponse['top_apps']
    let user_idle_s = 0
    let away_s = 0
    try {
      const gapStats = db.prepare(`
        SELECT
          SUM(CASE WHEN kind = 'user_idle'    THEN duration_s ELSE 0 END) AS user_idle_s,
          SUM(CASE WHEN kind = 'system_sleep' THEN duration_s ELSE 0 END) AS away_s
        FROM gaps
        WHERE started_at >= ? AND started_at < ?
      `).get(start, end) as { user_idle_s: number | null; away_s: number | null } | null
      user_idle_s = gapStats?.user_idle_s ?? 0
      away_s = gapStats?.away_s ?? 0
    } catch { /* gaps table not yet created by ETL */ }
    const categoryBreakdown = db.prepare(`
      SELECT category, SUM(duration_s) AS duration_s
      FROM app_sessions WHERE started_at >= ? AND started_at < ?
      GROUP BY category ORDER BY duration_s DESC
    `).all(start, end) as StatsResponse['category_breakdown']
    return { date, focus_s: t.focus_s ?? 0, user_idle_s, away_s, session_count: t.session_count, top_apps: topApps, category_breakdown: categoryBreakdown }
  } catch { return empty }
}

function getTimeline(date: string): TimelineResponse {
  try {
    const db = getDb()
    const { start, end } = localDayBounds(date)
    const rows = db.prepare(`
      SELECT id, app_name, started_at, ended_at, duration_s,
             window_titles, frame_count, etl_run_id,
             category, confidence
      FROM app_sessions WHERE started_at >= ? AND started_at < ?
      ORDER BY started_at ASC
    `).all(start, end) as Array<Record<string, unknown>>
    let gaps: GapRow[] = []
    try {
      gaps = db.prepare(`
        SELECT id, started_at, ended_at, duration_s, kind
        FROM gaps
        WHERE started_at >= ? AND started_at < ?
        ORDER BY started_at ASC
      `).all(start, end) as GapRow[]
    } catch { gaps = [] }
    const dayStartMs = new Date(`${date}T00:00:00`).getTime()
    const isToday = date === todayString()
    return {
      date,
      sessions: rows.map(parseSession),
      gaps,
      day_start_s: Math.floor(dayStartMs / 1000),
      day_end_s: Math.floor((isToday ? Date.now() : new Date(`${date}T23:59:59`).getTime()) / 1000),
    }
  } catch {
    return { date, sessions: [], gaps: [], day_start_s: Math.floor(Date.now() / 1000 - 3600), day_end_s: Math.floor(Date.now() / 1000) }
  }
}

function getRecentSessions(date: string): SessionRow[] {
  try {
    const db = getDb()
    const { start, end } = localDayBounds(date)
    const rows = db.prepare(`
      SELECT id, app_name, started_at, ended_at, duration_s,
             window_titles, audio_snippets, signals, frame_count, etl_run_id,
             category, confidence
      FROM app_sessions WHERE started_at >= ? AND started_at < ?
      ORDER BY started_at DESC LIMIT 8
    `).all(start, end) as Array<Record<string, unknown>>
    return rows.map(parseSession)
  } catch { return [] }
}

export default function DashboardPage() {
  const today = todayString()
  const active = getActiveSession()
  const stats = getStats(today)
  const timeline = getTimeline(today)
  const recent = getRecentSessions(today)

  return (
    <div className="space-y-6">
      <RefreshTrigger intervalMs={30_000} />

      {/* Header */}
      <div className="flex items-baseline justify-between">
        <h1 className="text-2xl font-semibold tracking-tight">{formatDateLabel(today)}</h1>
        {stats.session_count > 0 && (
          <span className="text-sm text-[#9B9A97]">{stats.session_count} sessions</span>
        )}
      </div>

      {/* Active session */}
      <ActiveSessionCard session={active} />

      {/* Day timeline */}
      {timeline.sessions.length > 0 && (
        <section>
          <p className="text-[10px] uppercase tracking-widest text-[#C8C6C1] mb-3">Timeline</p>
          <DayTimeline data={timeline} activeSession={active} />
        </section>
      )}

      {/* Stats */}
      {stats.session_count > 0 && <StatsRow stats={stats} />}

      {/* Category breakdown */}
      {stats.category_breakdown && stats.category_breakdown.length > 0 && (
        <section>
          <p className="text-[10px] uppercase tracking-widest text-[#C8C6C1] mb-3">By Category</p>
          <div className="rounded-2xl border border-[#E8E6E1] bg-white p-5">
            <CategoryBreakdown stats={stats.category_breakdown} />
          </div>
        </section>
      )}

      {/* Recent sessions */}
      {recent.length > 0 && (
        <section>
          <p className="text-[10px] uppercase tracking-widest text-[#C8C6C1] mb-3">Recent</p>
          <div className="space-y-2">
            {recent.map(s => <SessionCard key={s.id} session={s} />)}
          </div>
        </section>
      )}

      {stats.session_count === 0 && !active && (
        <div className="rounded-2xl border border-[#E8E6E1] bg-white px-6 py-12 text-center">
          <p className="text-[#9B9A97] text-sm">No activity recorded today</p>
          <p className="text-[#C8C6C1] text-xs mt-1">Run <code className="font-mono bg-[#F8F7F4] px-1.5 py-0.5 rounded text-[#9B9A97]">meridian</code> to start tracking</p>
        </div>
      )}
    </div>
  )
}
