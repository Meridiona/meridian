// meridian — AI activity intelligence by Meridiona

import { NextResponse } from 'next/server'
import getDb from '@/lib/db'
import { localDayBounds, todayString } from '@/lib/date-utils'
import type { TimelineResponse, SessionRow, GapRow } from '@/lib/types'

export const dynamic = 'force-dynamic'

function parseRow(r: Record<string, unknown>): SessionRow {
  return {
    id: r.id as number,
    app_name: r.app_name as string,
    started_at: r.started_at as string,
    ended_at: r.ended_at as string,
    duration_s: r.duration_s as number,
    window_titles: JSON.parse((r.window_titles as string) || '[]'),
    ocr_samples: r.ocr_samples ? JSON.parse(r.ocr_samples as string) : null,
    elements_samples: r.elements_samples ? JSON.parse(r.elements_samples as string) : null,
    audio_snippets: r.audio_snippets ? JSON.parse(r.audio_snippets as string) : null,
    signals: r.signals ? JSON.parse(r.signals as string) : null,
    frame_count: r.frame_count as number,
    etl_run_id: r.etl_run_id as number,
  }
}

export async function GET(request: Request) {
  const { searchParams } = new URL(request.url)
  const date = searchParams.get('date') ?? todayString()
  const { start, end } = localDayBounds(date)

  try {
    const db = getDb()
    const rows = db.prepare(`
      SELECT id, app_name, started_at, ended_at, duration_s,
             window_titles, ocr_samples, elements_samples,
             audio_snippets, signals, frame_count, etl_run_id
      FROM app_sessions
      WHERE started_at >= ? AND started_at < ?
      ORDER BY started_at ASC
    `).all(start, end) as Array<Record<string, unknown>>

    const sessions = rows.map(parseRow)

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
    const dayEndMs = isToday ? Date.now() : new Date(`${date}T23:59:59`).getTime()

    const response: TimelineResponse = {
      date,
      sessions,
      gaps,
      day_start_s: Math.floor(dayStartMs / 1000),
      day_end_s: Math.floor(dayEndMs / 1000),
    }

    return NextResponse.json(response)
  } catch (e) {
    console.error('timeline error:', e)
    const dayStartMs = new Date(`${date}T00:00:00`).getTime()
    return NextResponse.json({
      date,
      sessions: [],
      gaps: [],
      day_start_s: Math.floor(dayStartMs / 1000),
      day_end_s: Math.floor(Date.now() / 1000),
    })
  }
}
