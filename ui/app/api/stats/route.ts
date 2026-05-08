// meridian — AI activity intelligence by Meridiona

import { NextResponse } from 'next/server'
import getDb from '@/lib/db'
import { localDayBounds, todayString } from '@/lib/date-utils'
import type { StatsResponse } from '@/lib/types'

export const dynamic = 'force-dynamic'

export async function GET(request: Request) {
  const { searchParams } = new URL(request.url)
  const date = searchParams.get('date') ?? todayString()
  const { start, end } = localDayBounds(date)

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

    const response: StatsResponse = {
      date,
      focus_s: totals.focus_s ?? 0,
      user_idle_s,
      away_s,
      session_count: totals.session_count,
      top_apps: topApps,
    }

    return NextResponse.json(response)
  } catch (e) {
    console.error('stats error:', e)
    return NextResponse.json({ date, focus_s: 0, user_idle_s: 0, away_s: 0, session_count: 0, top_apps: [] })
  }
}
