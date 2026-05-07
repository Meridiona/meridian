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
        SUM(CASE WHEN app_name != '(idle)' THEN duration_s ELSE 0 END) AS focus_s,
        SUM(CASE WHEN app_name = '(idle)'  THEN duration_s ELSE 0 END) AS idle_s,
        COUNT(*) AS session_count
      FROM app_sessions
      WHERE started_at >= ? AND started_at < ?
    `).get(start, end) as { focus_s: number | null; idle_s: number | null; session_count: number }

    const topApps = db.prepare(`
      SELECT app_name, SUM(duration_s) AS duration_s, COUNT(*) AS session_count
      FROM app_sessions
      WHERE started_at >= ? AND started_at < ? AND app_name != '(idle)'
      GROUP BY app_name
      ORDER BY duration_s DESC
      LIMIT 8
    `).all(start, end) as Array<{ app_name: string; duration_s: number; session_count: number }>

    const response: StatsResponse = {
      date,
      total_s: (totals.focus_s ?? 0) + (totals.idle_s ?? 0),
      focus_s: totals.focus_s ?? 0,
      idle_s: totals.idle_s ?? 0,
      session_count: totals.session_count,
      top_apps: topApps,
    }

    return NextResponse.json(response)
  } catch (e) {
    console.error('stats error:', e)
    return NextResponse.json({ date, total_s: 0, focus_s: 0, idle_s: 0, session_count: 0, top_apps: [] })
  }
}
