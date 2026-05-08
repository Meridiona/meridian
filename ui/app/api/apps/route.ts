import { NextResponse } from 'next/server'
import getDb from '@/lib/db'
import type { AppStat } from '@/lib/types'

export const dynamic = 'force-dynamic'

export async function GET() {
  try {
    const db = getDb()

    const rows = db.prepare(`
      SELECT
        app_name,
        SUM(duration_s) AS total_s,
        COUNT(*) AS session_count,
        CAST(AVG(duration_s) AS INTEGER) AS avg_session_s,
        MAX(ended_at) AS last_seen
      FROM app_sessions
      GROUP BY app_name
      ORDER BY total_s DESC
    `).all() as AppStat[]

    return NextResponse.json(rows)
  } catch (e) {
    console.error('apps error:', e)
    return NextResponse.json([])
  }
}
