// meridian — AI activity intelligence by Meridiona

import { NextResponse } from 'next/server'
import getDb from '@/lib/db'
import { localDayBounds, todayString } from '@/lib/date-utils'
import { logger, withSpan } from '@/lib/observability'
import type { StatsResponse } from '@/lib/types'

export const dynamic = 'force-dynamic'

export async function GET(request: Request) {
  const url = new URL(request.url)
  const date = url.searchParams.get('date') ?? todayString()
  const { start, end } = localDayBounds(date)

  return withSpan('api.stats', { route: url.pathname, date }, async () => {
  try {
    const db = getDb()

    const totals = db.prepare(`
      SELECT
        SUM(duration_s) AS focus_s,
        COUNT(*) AS session_count
      FROM app_sessions
      WHERE started_at >= ? AND started_at < ?
    `).get(start, end) as { focus_s: number | null; session_count: number }

    const topApps = db.prepare(`
      SELECT app_name, SUM(duration_s) AS duration_s, COUNT(*) AS session_count
      FROM app_sessions
      WHERE started_at >= ? AND started_at < ?
      GROUP BY app_name
      ORDER BY duration_s DESC
      LIMIT 8
    `).all(start, end) as Array<{ app_name: string; duration_s: number; session_count: number }>

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

    const category_breakdown = db.prepare(`
      SELECT category, SUM(duration_s) AS duration_s
      FROM app_sessions
      WHERE started_at >= ? AND started_at < ?
      GROUP BY category
      ORDER BY duration_s DESC
    `).all(start, end) as Array<{ category: string; duration_s: number }>

    const response: StatsResponse = {
      date,
      focus_s: totals.focus_s ?? 0,
      user_idle_s,
      away_s,
      session_count: totals.session_count,
      top_apps: topApps,
      category_breakdown,
    }

    return NextResponse.json(response)
  } catch (e) {
    logger.error({ err: e instanceof Error ? e.message : String(e), route: 'stats' }, 'stats handler failed')
    return NextResponse.json({ date, focus_s: 0, user_idle_s: 0, away_s: 0, session_count: 0, top_apps: [] })
  }
  })
}
