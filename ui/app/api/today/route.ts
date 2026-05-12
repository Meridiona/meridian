// meridian — normalises screenpipe activity into structured app sessions

import { NextResponse } from 'next/server'
import getDb from '@/lib/db'
import { localDayBounds, todayString } from '@/lib/date-utils'

export const dynamic = 'force-dynamic'

interface TodaySession {
  id: number
  app: string
  started_at: string
  dur: number
  cat: string
  titles: string[]
  explain: string | null
  routing: string | null
  task_key: string | null
  candidates: string[]
  confidence: number
  method: string
}

interface TodayActive {
  app: string
  started_at: string
  elapsed_s: number
  cat: string
  titles: string[]
  confidence: number
  explain: string | null
}

interface TodayGap {
  id: number
  kind: string
  started_at: string
  ended_at: string
  dur: number
}

export interface TodayResponse {
  date: string
  sessions: TodaySession[]
  active: TodayActive | null
  gaps: TodayGap[]
  focus_s: number
  idle_s: number
  session_count: number
}

export async function GET() {
  const date = todayString()
  const { start, end } = localDayBounds(date)

  try {
    const db = getDb()

    // category_explanation was added in migration 009 — check gracefully
    let hasExplanation = false
    try { db.prepare('SELECT category_explanation FROM app_sessions LIMIT 0').run(); hasExplanation = true } catch { /* pre-009 */ }

    const sql = `
      SELECT
        s.id,
        s.app_name,
        s.started_at,
        s.duration_s,
        s.category,
        s.confidence,
        s.category_method,
        ${hasExplanation ? 's.category_explanation,' : "NULL AS category_explanation,"}
        s.window_titles,
        tl.task_key,
        tl.routing,
        tl.session_type
      FROM app_sessions s
      LEFT JOIN ticket_links tl ON tl.session_id = s.id
      WHERE s.started_at >= ? AND s.started_at < ?
      ORDER BY s.started_at ASC
    `
    const rows = db.prepare(sql).all(start, end) as Array<Record<string, unknown>>

    const sessions: TodaySession[] = rows.map(r => {
      const titles: Array<{ window_name?: string; title?: string; count: number }> =
        JSON.parse((r.window_titles as string) || '[]')
      const topTitle = titles[0]?.window_name ?? titles[0]?.title ?? (r.app_name as string)

      // parse candidate keys from explain or just use task_key
      const candidates = r.task_key ? [r.task_key as string] : []

      return {
        id: r.id as number,
        app: r.app_name as string,
        started_at: r.started_at as string,
        dur: r.duration_s as number,
        cat: (r.category as string) || 'idle_personal',
        titles: titles.length ? titles.map(t => t.window_name ?? t.title ?? '').filter(Boolean) : [topTitle],
        explain: (r.category_explanation as string) || null,
        routing: (r.routing as string) || null,
        task_key: (r.task_key as string) || null,
        candidates,
        confidence: (r.confidence as number) || 0,
        method: (r.category_method as string) || 'rule_based',
      }
    })

    // active session
    let active: TodayActive | null = null
    try {
      const activeExplCol = hasExplanation ? 'category_explanation' : "NULL AS category_explanation"
      const ar = db.prepare(`
        SELECT app_name, started_at, last_seen_at, window_titles, category, confidence, ${activeExplCol}
        FROM active_session WHERE id = 1
      `).get() as Record<string, unknown> | undefined

      if (ar) {
        const titles: Array<{ window_name?: string; title?: string; count: number }> =
          JSON.parse((ar.window_titles as string) || '[]')
        active = {
          app: ar.app_name as string,
          started_at: ar.started_at as string,
          elapsed_s: Math.floor((Date.now() - new Date(ar.started_at as string).getTime()) / 1000),
          cat: (ar.category as string) || 'idle_personal',
          titles: titles.map(t => t.window_name ?? t.title ?? '').filter(Boolean),
          confidence: (ar.confidence as number) || 0,
          explain: (ar.category_explanation as string) || null,
        }
      }
    } catch { /* no active session */ }

    // gaps
    const gaps: TodayGap[] = []
    try {
      const gRows = db.prepare(`
        SELECT id, kind, started_at, ended_at, duration_s
        FROM gaps WHERE started_at >= ? AND started_at < ?
        ORDER BY started_at ASC
      `).all(start, end) as Array<Record<string, unknown>>
      gRows.forEach(g => gaps.push({
        id: g.id as number,
        kind: g.kind as string,
        started_at: g.started_at as string,
        ended_at: g.ended_at as string,
        dur: g.duration_s as number,
      }))
    } catch { /* gaps table might not exist */ }

    const focus_s = sessions.reduce((a, s) => a + s.dur, 0) + (active?.elapsed_s ?? 0)
    const idle_s = gaps.filter(g => g.kind === 'user_idle').reduce((a, g) => a + g.dur, 0)

    const resp: TodayResponse = {
      date,
      sessions,
      active,
      gaps,
      focus_s,
      idle_s,
      session_count: sessions.length + (active ? 1 : 0),
    }

    return NextResponse.json(resp)
  } catch (e) {
    console.error('today api error:', e)
    return NextResponse.json({
      date, sessions: [], active: null, gaps: [],
      focus_s: 0, idle_s: 0, session_count: 0,
    })
  }
}
