// meridian — AI activity intelligence by Meridiona

import { NextResponse } from 'next/server'
import getDb from '@/lib/db'
import type { ActiveSessionRow } from '@/lib/types'

export const dynamic = 'force-dynamic'

export async function GET() {
  try {
    const db = getDb()
    const row = db.prepare(`
      SELECT app_name, started_at, last_seen_at,
             window_titles, ocr_samples, audio_snippets, signals, frame_count,
             category, confidence
      FROM active_session WHERE id = 1
    `).get() as Record<string, unknown> | undefined

    if (!row) return NextResponse.json(null)

    const elapsed_s = Math.floor(
      (Date.now() - new Date(row.started_at as string).getTime()) / 1000
    )

    const result: ActiveSessionRow = {
      app_name: row.app_name as string,
      started_at: row.started_at as string,
      last_seen_at: row.last_seen_at as string,
      window_titles: JSON.parse((row.window_titles as string) || '[]'),
      ocr_samples: row.ocr_samples ? JSON.parse(row.ocr_samples as string) : null,
      audio_snippets: row.audio_snippets ? JSON.parse(row.audio_snippets as string) : null,
      signals: row.signals ? JSON.parse(row.signals as string) : null,
      frame_count: row.frame_count as number,
      elapsed_s,
      category: (row.category as string) || 'idle_personal',
      confidence: (row.confidence as number) || 0,
    }

    return NextResponse.json(result)
  } catch {
    return NextResponse.json(null)
  }
}
