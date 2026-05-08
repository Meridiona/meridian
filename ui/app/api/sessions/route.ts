// meridian — AI activity intelligence by Meridiona

import { NextResponse } from 'next/server'
import getDb from '@/lib/db'
import { localDayBounds, todayString } from '@/lib/date-utils'
import type { PaginatedSessions, SessionRow } from '@/lib/types'

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
  const appFilter = searchParams.get('app')
  const page = Math.max(1, parseInt(searchParams.get('page') ?? '1'))
  const pageSize = Math.min(50, parseInt(searchParams.get('page_size') ?? '20'))
  const offset = (page - 1) * pageSize

  const { start, end } = localDayBounds(date)

  try {
    const db = getDb()

    const appCondition = appFilter ? 'AND app_name = ?' : ''
    const params = appFilter
      ? [start, end, appFilter, pageSize, offset]
      : [start, end, pageSize, offset]
    const countParams = appFilter ? [start, end, appFilter] : [start, end]

    const total = (db.prepare(`
      SELECT COUNT(*) AS n FROM app_sessions
      WHERE started_at >= ? AND started_at < ? ${appCondition}
    `).get(...countParams) as { n: number }).n

    const rows = db.prepare(`
      SELECT id, app_name, started_at, ended_at, duration_s,
             window_titles, ocr_samples, elements_samples,
             audio_snippets, signals, frame_count, etl_run_id
      FROM app_sessions
      WHERE started_at >= ? AND started_at < ? ${appCondition}
      ORDER BY started_at DESC
      LIMIT ? OFFSET ?
    `).all(...params) as Array<Record<string, unknown>>

    const response: PaginatedSessions = {
      sessions: rows.map(parseRow),
      page,
      page_size: pageSize,
      total,
      has_more: offset + rows.length < total,
    }

    return NextResponse.json(response)
  } catch (e) {
    console.error('sessions error:', e)
    return NextResponse.json({ sessions: [], page, page_size: pageSize, total: 0, has_more: false })
  }
}
