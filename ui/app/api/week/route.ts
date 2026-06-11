//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

import { NextResponse } from 'next/server'
import getDb from '@/lib/db'

export const dynamic = 'force-dynamic'

interface DaySummary {
  day: string
  date: string
  total_s: number
  cats: Record<string, number>
  isToday: boolean
}

export interface WeekResponse {
  days: DaySummary[]
  total_s: number
}

function localDayRange(dateStr: string): { start: string; end: string } {
  return {
    start: `${dateStr}T00:00:00`,
    end:   `${dateStr}T23:59:59.999`,
  }
}

export async function GET() {
  try {
    const db = getDb()
    const todayMs = Date.now()
    const todayStr = new Date(todayMs).toLocaleDateString('en-CA') // YYYY-MM-DD

    const days: DaySummary[] = []
    for (let i = 6; i >= 0; i--) {
      const d = new Date(todayMs - i * 86400000)
      const dateStr = d.toLocaleDateString('en-CA')
      const dow = d.toLocaleDateString('en-US', { weekday: 'short' })
      const mmdd = d.toLocaleDateString('en-US', { month: 'numeric', day: 'numeric' })
      const { start, end } = localDayRange(dateStr)

      // Foreground stream only. The coding-agent transcript overlay
      // (claude_session_uuid IS NOT NULL) records the same wall-clock time a
      // second time, so including it would double-count each day's total and
      // inflate the `coding` band. Foreground sessions never overlap each other,
      // so SUM here already equals the day's true union.
      const rows = db.prepare(`
        SELECT category, SUM(duration_s) AS dur_s
        FROM app_sessions
        WHERE started_at >= ? AND started_at < ?
          AND claude_session_uuid IS NULL
        GROUP BY category
      `).all(start, end) as Array<{ category: string; dur_s: number }>

      const cats: Record<string, number> = {}
      let total_s = 0
      rows.forEach(r => {
        const h = r.dur_s / 3600
        cats[r.category] = (cats[r.category] ?? 0) + h
        total_s += r.dur_s
      })

      // include active session hours for today
      if (dateStr === todayStr) {
        try {
          const ar = db.prepare(`SELECT started_at, category FROM active_session WHERE id = 1`).get() as
            Record<string, unknown> | undefined
          if (ar) {
            const elapsed = Math.floor((Date.now() - new Date(ar.started_at as string).getTime()) / 1000)
            const cat = (ar.category as string) || 'idle_personal'
            cats[cat] = (cats[cat] ?? 0) + elapsed / 3600
            total_s += elapsed
          }
        } catch { /* no active */ }
      }

      days.push({ day: dow, date: mmdd, total_s, cats, isToday: dateStr === todayStr })
    }

    const total_s = days.reduce((a, d) => a + d.total_s, 0)
    return NextResponse.json({ days, total_s })
  } catch (e) {
    console.error('week api error:', e)
    return NextResponse.json({ days: [], total_s: 0 })
  }
}
