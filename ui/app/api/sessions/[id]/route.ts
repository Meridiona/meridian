// meridian — AI activity intelligence by Meridiona

import { NextResponse } from 'next/server'
import getDb from '@/lib/db'
import type { SessionRow } from '@/lib/types'

export const dynamic = 'force-dynamic'

interface DimensionRow {
  dimension: string
  value: string
  confidence: number
  source: string
  created_at: string
}

interface DispatchRow {
  id: number
  task_key: string
  provider: string
  state: string
  attempts: number
  last_error: string | null
  payload: Record<string, unknown> | null
  created_at: string
  dispatched_at: string | null
}

export interface SessionDetailResponse {
  session: SessionRow
  dimensions: DimensionRow[]
  dispatches: DispatchRow[]
  summary_json: Record<string, unknown> | null
}

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

export async function GET(
  _request: Request,
  context: { params: Promise<{ id: string }> },
) {
  const { id: idStr } = await context.params
  const id = Number(idStr)
  if (!Number.isFinite(id) || id <= 0) {
    return NextResponse.json({ error: 'invalid id' }, { status: 400 })
  }

  try {
    const db = getDb()

    const sessionRow = db.prepare(`
      SELECT s.id, s.app_name, s.started_at, s.ended_at, s.duration_s,
             s.window_titles, s.audio_snippets, s.signals, s.frame_count, s.etl_run_id,
             s.category, s.confidence,
             s.task_key,
             s.task_session_type AS session_type,
             s.task_routing      AS routing,
             s.task_confidence   AS link_confidence,
             s.task_method       AS link_method,
             pt.title            AS task_title,
             pt.url              AS task_url,
             pt.provider         AS task_provider
        FROM app_sessions s
        LEFT JOIN pm_tasks pt ON pt.task_key = s.task_key
       WHERE s.id = ?
    `).get(id) as Record<string, unknown> | undefined

    if (!sessionRow) {
      return NextResponse.json({ error: 'session not found' }, { status: 404 })
    }

    let dimensions: DimensionRow[] = []
    try {
      dimensions = db.prepare(`
        SELECT dimension, value, confidence, source, created_at
          FROM session_dimensions
         WHERE session_id = ?
         ORDER BY dimension, confidence DESC
      `).all(id) as DimensionRow[]
    } catch { /* table not present yet */ }

    let dispatches: DispatchRow[] = []
    try {
      const rows = db.prepare(`
        SELECT id, task_key, provider, state, attempts, last_error,
               payload_json AS payload, created_at, dispatched_at
          FROM dispatch_queue
         WHERE session_id = ?
         ORDER BY created_at DESC
      `).all(id) as Array<Record<string, unknown>>
      dispatches = rows.map(r => ({
        id: r.id as number,
        task_key: r.task_key as string,
        provider: r.provider as string,
        state: r.state as string,
        attempts: Number(r.attempts ?? 0),
        last_error: (r.last_error as string | null) ?? null,
        payload: r.payload ? JSON.parse(r.payload as string) : null,
        created_at: r.created_at as string,
        dispatched_at: (r.dispatched_at as string | null) ?? null,
      }))
    } catch { /* table not present yet */ }

    let summary_json: Record<string, unknown> | null = null
    try {
      const sumRow = db.prepare(`
        SELECT summary_json FROM session_summaries WHERE session_id = ?
      `).get(id) as { summary_json: string } | undefined
      if (sumRow?.summary_json) {
        try { summary_json = JSON.parse(sumRow.summary_json) } catch { /* ignore */ }
      }
    } catch { /* table not present yet */ }

    const response: SessionDetailResponse = {
      session: parseRow(sessionRow),
      dimensions,
      dispatches,
      summary_json,
    }
    return NextResponse.json(response)
  } catch (e) {
    console.error('session detail error:', e)
    return NextResponse.json({ error: 'internal error' }, { status: 500 })
  }
}
