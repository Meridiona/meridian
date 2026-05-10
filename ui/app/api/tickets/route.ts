// meridian — AI activity intelligence by Meridiona

import { NextResponse } from 'next/server'
import getDb from '@/lib/db'
import { localDayBounds, todayString } from '@/lib/date-utils'

export const dynamic = 'force-dynamic'

export interface TicketBreakdownEntry {
  task_key: string
  title: string | null
  url: string | null
  provider: string | null
  duration_s: number
  session_count: number
}

export interface TicketsTodayResponse {
  date: string
  tasks: TicketBreakdownEntry[]
  overhead_s: number
  untagged_s: number
  total_tagged_s: number
}

export async function GET(request: Request) {
  const { searchParams } = new URL(request.url)
  const date = searchParams.get('date') ?? todayString()
  const { start, end } = localDayBounds(date)

  try {
    const db = getDb()

    // Per-task minutes today.
    const taskRows = db.prepare(`
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

    // Overhead today (session_type = 'overhead').
    const overheadRow = db.prepare(`
      SELECT COALESCE(SUM(s.duration_s), 0) AS s
        FROM app_sessions s
        JOIN ticket_links tl ON tl.session_id = s.id
       WHERE s.started_at >= ? AND s.started_at < ?
         AND tl.session_type = 'overhead'
    `).get(start, end) as { s: number }

    // Untagged today (no ticket_links row at all).
    const untaggedRow = db.prepare(`
      SELECT COALESCE(SUM(s.duration_s), 0) AS s
        FROM app_sessions s
        LEFT JOIN ticket_links tl ON tl.session_id = s.id
       WHERE s.started_at >= ? AND s.started_at < ?
         AND tl.session_id IS NULL
    `).get(start, end) as { s: number }

    const tasks: TicketBreakdownEntry[] = taskRows.map(r => ({
      task_key: r.task_key as string,
      title:    (r.title as string | null) ?? null,
      url:      (r.url as string | null) ?? null,
      provider: (r.provider as string | null) ?? null,
      duration_s: Number(r.duration_s ?? 0),
      session_count: Number(r.session_count ?? 0),
    }))

    const total_tagged_s = tasks.reduce((acc, t) => acc + t.duration_s, 0)

    const response: TicketsTodayResponse = {
      date,
      tasks,
      overhead_s:    Number(overheadRow?.s ?? 0),
      untagged_s:    Number(untaggedRow?.s ?? 0),
      total_tagged_s,
    }

    return NextResponse.json(response)
  } catch (e) {
    console.error('tickets error:', e)
    return NextResponse.json({
      date,
      tasks: [],
      overhead_s: 0,
      untagged_s: 0,
      total_tagged_s: 0,
    } as TicketsTodayResponse)
  }
}
