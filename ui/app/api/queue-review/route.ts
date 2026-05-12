// meridian — normalises screenpipe activity into structured app sessions

import { NextResponse } from 'next/server'
import getDb from '@/lib/db'
import { localDayBounds, todayString } from '@/lib/date-utils'

export const dynamic = 'force-dynamic'

export interface QueueItem {
  id: number
  app: string
  started_at: string
  dur: number
  cat: string
  titles: string[]
  explain: string | null
  candidates: string[]
}

export interface QueueResponse {
  items: QueueItem[]
}

export async function GET() {
  const today = todayString()
  const { start, end } = localDayBounds(today)

  try {
    const db = getDb()

    let hasExplanation = false
    try { db.prepare('SELECT category_explanation FROM app_sessions LIMIT 0').run(); hasExplanation = true } catch { /* pre-009 */ }

    const explCol = hasExplanation ? 's.category_explanation' : 'NULL AS category_explanation'

    const rows = db.prepare(`
      SELECT
        s.id,
        s.app_name,
        s.started_at,
        s.duration_s,
        s.category,
        ${explCol},
        s.window_titles,
        s.confidence,
        tl.task_key,
        tl.routing
      FROM app_sessions s
      LEFT JOIN ticket_links tl ON tl.session_id = s.id
      WHERE s.started_at >= ? AND s.started_at < ?
        AND (
          tl.routing = 'queue'
          OR (tl.id IS NULL AND s.confidence < 0.6 AND s.category_method = 'foundation_models')
        )
      ORDER BY s.started_at DESC
      LIMIT 50
    `).all(start, end) as Array<Record<string, unknown>>

    // get task titles for candidates
    const allTaskKeys = new Set<string>()
    rows.forEach(r => { if (r.task_key) allTaskKeys.add(r.task_key as string) })

    const items: QueueItem[] = rows.map(r => {
      const titles: Array<{ window_name?: string; title?: string; count: number }> =
        JSON.parse((r.window_titles as string) || '[]')
      const candidates = r.task_key ? [r.task_key as string] : []
      return {
        id: r.id as number,
        app: r.app_name as string,
        started_at: r.started_at as string,
        dur: r.duration_s as number,
        cat: (r.category as string) || 'idle_personal',
        titles: titles.map(t => t.window_name ?? t.title ?? '').filter(Boolean),
        explain: (r.category_explanation as string) || null,
        candidates,
      }
    })

    return NextResponse.json({ items })
  } catch (e) {
    console.error('queue-review api error:', e)
    return NextResponse.json({ items: [] })
  }
}
