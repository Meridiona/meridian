// meridian — normalises screenpipe activity into structured app sessions
'use client'

import getDb from '@/lib/db'
import { localDayBounds, todayString } from '@/lib/date-utils'
import { formatDateLabel } from '@/lib/format'
import ActiveSessionCard from '@/components/ActiveSessionCard'
import DayTimeline from '@/components/DayTimeline'
import StatsRow from '@/components/StatsRow'
import SessionCard from '@/components/SessionCard'
import RefreshTrigger from '@/components/RefreshTrigger'
import CategoryBreakdown from '@/components/CategoryBreakdown'
import TicketBreakdown from '@/components/TicketBreakdown'
import type {
  ActiveSessionRow, StatsResponse, TimelineResponse, SessionRow, GapRow
} from '@/lib/types'

// Lazy-load heavy views
const TodayView    = dynamic(() => import('@/components/views/TodayView'),    { ssr: false })
const TasksView    = dynamic(() => import('@/components/views/TasksView'),    { ssr: false })
const QueueView    = dynamic(() => import('@/components/views/QueueView'),    { ssr: false })
const SessionsView = dynamic(() => import('@/components/views/SessionsView'), { ssr: false })
const WeekView     = dynamic(() => import('@/components/views/WeekView'),     { ssr: false })

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
    task_key:        (r.task_key as string | null) ?? null,
    task_title:      (r.task_title as string | null) ?? null,
    task_url:        (r.task_url as string | null) ?? null,
    task_provider:   (r.task_provider as string | null) ?? null,
    session_type:    (r.session_type as string | null) ?? null,
    routing:         (r.routing as string | null) ?? null,
    link_confidence: (r.link_confidence as number | null) ?? null,
    link_method:     (r.link_method as string | null) ?? null,
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

export default function DashboardPage() {
  const [view, setView] = useState<View>('today')
  const [focusKey, setFocusKey] = useState<string | null>(null)
  const [cmdOpen, setCmdOpen] = useState(false)
  const [queueCount, setQueueCount] = useState(0)

  const navigate = useCallback((v: View, key?: string) => {
    setView(v)
    setFocusKey(key ?? null)
    window.scrollTo({ top: 0, behavior: 'instant' })
  }, [])

  // fetch queue count for sidebar badge
  useEffect(() => {
    fetch('/api/queue-review')
      .then(r => r.json())
      .then(d => setQueueCount(d.items?.length ?? 0))
  }, [])

  // keyboard shortcuts
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
        e.preventDefault()
        setCmdOpen(o => !o)
        return
      }
      if (e.key === 'Escape') { setCmdOpen(false); return }
      if (cmdOpen || e.metaKey || e.ctrlKey || e.altKey) return
      const target = e.target as HTMLElement
      if (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA') return
      if (e.key === '1') navigate('today')
      else if (e.key === '2') navigate('tasks')
      else if (e.key === '3') navigate('queue')
      else if (e.key === '4') navigate('sessions')
      else if (e.key === '5') navigate('week')
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [cmdOpen, navigate])

interface TicketsBundle {
  tasks: Array<{
    task_key: string
    title: string | null
    url: string | null
    provider: string | null
    duration_s: number
    session_count: number
  }>
  overhead_s: number
  untagged_s: number
}

function getTickets(date: string): TicketsBundle {
  try {
    const db = getDb()
    const { start, end } = localDayBounds(date)
    const tasks = db.prepare(`
      SELECT tl.task_key                                AS task_key,
             pt.title                                   AS title,
             pt.url                                     AS url,
             pt.provider                                AS provider,
             SUM(s.duration_s)                          AS duration_s,
             COUNT(*)                                   AS session_count
        FROM app_sessions s
        JOIN ticket_links tl ON tl.session_id = s.id AND tl.session_type = 'task'
        LEFT JOIN pm_tasks pt ON pt.task_key  = tl.task_key
       WHERE s.started_at >= ? AND s.started_at < ?
         AND tl.task_key IS NOT NULL
       GROUP BY tl.task_key
       ORDER BY duration_s DESC
    `).all(start, end) as Array<Record<string, unknown>>

    const overhead = (db.prepare(`
      SELECT COALESCE(SUM(s.duration_s), 0) AS s
        FROM app_sessions s
        JOIN ticket_links tl ON tl.session_id = s.id
       WHERE s.started_at >= ? AND s.started_at < ?
         AND tl.session_type = 'overhead'
    `).get(start, end) as { s: number }).s

    const untagged = (db.prepare(`
      SELECT COALESCE(SUM(s.duration_s), 0) AS s
        FROM app_sessions s
        LEFT JOIN ticket_links tl ON tl.session_id = s.id
       WHERE s.started_at >= ? AND s.started_at < ?
         AND tl.session_id IS NULL
    `).get(start, end) as { s: number }).s

    return {
      tasks: tasks.map(r => ({
        task_key: r.task_key as string,
        title: (r.title as string | null) ?? null,
        url:   (r.url as string | null) ?? null,
        provider: (r.provider as string | null) ?? null,
        duration_s: Number(r.duration_s ?? 0),
        session_count: Number(r.session_count ?? 0),
      })),
      overhead_s: Number(overhead ?? 0),
      untagged_s: Number(untagged ?? 0),
    }
  } catch {
    return { tasks: [], overhead_s: 0, untagged_s: 0 }
  }
}

function getRecentSessions(date: string): SessionRow[] {
  try {
    const db = getDb()
    const { start, end } = localDayBounds(date)
    const rows = db.prepare(`
      SELECT s.id, s.app_name, s.started_at, s.ended_at, s.duration_s,
             s.window_titles, s.ocr_samples, s.elements_samples,
             s.audio_snippets, s.signals, s.frame_count, s.etl_run_id,
             s.category, s.confidence,
             tl.task_key       AS task_key,
             tl.session_type   AS session_type,
             tl.routing        AS routing,
             tl.confidence     AS link_confidence,
             tl.method         AS link_method,
             pt.title          AS task_title,
             pt.url            AS task_url,
             pt.provider       AS task_provider
        FROM app_sessions s
        LEFT JOIN ticket_links tl ON tl.session_id = s.id
        LEFT JOIN pm_tasks    pt ON pt.task_key   = tl.task_key
       WHERE s.started_at >= ? AND s.started_at < ?
       ORDER BY s.started_at DESC LIMIT 8
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
  const tickets = getTickets(today)

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

      {/* Today's tickets */}
      {(tickets.tasks.length > 0 || tickets.overhead_s > 0 || tickets.untagged_s > 0) && (
        <section>
          <p className="text-[10px] uppercase tracking-widest text-[#C8C6C1] mb-3">Today&rsquo;s Tickets</p>
          <div className="rounded-2xl border border-[#E8E6E1] bg-white p-5">
            <TicketBreakdown
              tasks={tickets.tasks}
              overhead_s={tickets.overhead_s}
              untagged_s={tickets.untagged_s}
            />
          </div>
        </section>
      )}

      {/* Category breakdown */}
      {stats.category_breakdown && stats.category_breakdown.length > 0 && (
        <section>
          <p className="text-[10px] uppercase tracking-widest text-[#C8C6C1] mb-3">By Category</p>
          <div className="rounded-2xl border border-[#E8E6E1] bg-white p-5">
            <CategoryBreakdown stats={stats.category_breakdown} />
          </div>
        </section>
      )}

      {cmdOpen && (
        <CommandBar
          onClose={() => setCmdOpen(false)}
          onNavigate={(v, k) => { navigate(v, k); setCmdOpen(false) }}
        />
      )}

      <TweaksPanel />
    </div>
  )
}
