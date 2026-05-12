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

    // LEFT JOIN ticket_links + pm_tasks so the UI can show the task pill
    // next to each session. The joins are optional — if migrations 003/005
    // haven't run yet (no ticket_links / pm_tasks tables), we fall back to
    // a session-only query in the catch below.
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
       WHERE s.started_at >= ? AND s.started_at < ? ${appCondition.replace(/app_name/g, 's.app_name')}
       ORDER BY s.started_at DESC
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
