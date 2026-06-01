// meridian — normalises screenpipe activity into structured app sessions

import { NextResponse } from 'next/server'
import getDb from '@/lib/db'
import { localDayBounds, todayString } from '@/lib/date-utils'
import { unionSeconds, intersectSeconds, mergeIntervals, countSwitches, type Interval } from '@/lib/intervals'

export const dynamic = 'force-dynamic'

/**
 * Foreground sessions shorter than this are sub-second focus jitter from
 * screenpipe, not real context switches — excluded from the switch count.
 */
const SWITCH_MIN_DURATION_S = 15

/** ISO timestamp `secs` seconds after `iso` — caps an agent transcript span to
 * its engaged `duration_s` so parked-open sessions don't masquerade as activity. */
function addSeconds(iso: string, secs: number): string {
  return new Date(new Date(iso).getTime() + Math.max(0, secs) * 1000).toISOString()
}

interface TodaySession {
  id: number
  app: string
  started_at: string
  dur: number
  cat: string
  titles: string[]
  explain: string | null
  routing: string | null
  session_type: string | null
  task_key: string | null
  candidates: string[]
  confidence: number
  method: string
  link_method: string | null
  link_confidence: number | null
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
  // ── Presence (mutually exclusive: you were either active or idle) ──────────
  focus_s: number        // ACTIVE presence — union of foreground sessions you were engaged in
  idle_s: number         // away from keyboard (user_idle gaps)
  // ── Agent overlay (a layer ON TOP of presence, never additive to focus) ────
  agent_s: number        // engaged coding-agent time (capped to duration_s, unioned)
  supervised_s: number   // agent time that ran WHILE you were active (AI-assisted) — subset of focus_s
  autonomous_s: number   // agent time that ran while you were away (agent_s − supervised_s)
  // ── Timeline bands ─────────────────────────────────────────────────────────
  presence_segments: Interval[] // merged active blocks (foreground), for the day timeline
  agent_segments: Interval[]    // merged engaged-agent blocks, drawn as an overlay band
  // ── Counts ───────────────────────────────────────────────────────────────
  session_count: number  // foreground sessions only
  switch_count: number   // genuine context switches in the foreground stream
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
        s.ended_at,
        s.duration_s,
        s.claude_session_uuid,
        s.category,
        s.confidence,
        s.category_method,
        ${hasExplanation ? 's.category_explanation,' : "NULL AS category_explanation,"}
        s.window_titles,
        s.task_key,
        s.task_routing      AS routing,
        s.task_session_type AS session_type,
        s.task_method       AS link_method,
        s.task_confidence   AS link_confidence
      FROM app_sessions s
      WHERE s.started_at >= ? AND s.started_at < ?
      ORDER BY s.started_at ASC
    `
    const allRows = db.prepare(sql).all(start, end) as Array<Record<string, unknown>>

    // Two streams share app_sessions: the foreground screen-capture stream
    // (claude_session_uuid IS NULL) and the coding-agent transcript overlay
    // (claude_session_uuid IS NOT NULL). The overlay records the same wall-clock
    // time from a second angle, so it must NOT appear as its own foreground
    // session — it drives the unioned focus figure and the coding-agent tile,
    // never the per-task buckets, timeline, or switch count.
    const rows = allRows.filter(r => r.claude_session_uuid == null)
    const codingRows = allRows.filter(r => r.claude_session_uuid != null)

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
        cat: (['fm_parse_error', 'fm_skip'].includes(r.category as string) ? 'idle_personal' : (r.category as string)) || 'idle_personal',
        titles: titles.length ? titles.map(t => t.window_name ?? t.title ?? '').filter(Boolean) : [topTitle],
        explain: (r.category_explanation as string) || null,
        routing: (r.routing as string) || null,
        session_type: (r.session_type as string) || null,
        task_key: (r.task_key as string) || null,
        candidates,
        confidence: (r.confidence as number) || 0,
        method: (r.category_method as string) || 'rule_based',
        link_method: (r.link_method as string) || null,
        link_confidence: typeof r.link_confidence === 'number' ? r.link_confidence : null,
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

    const nowIso = new Date().toISOString()

    // ── Presence (the foreground stream — where you were demonstrably active) ──
    // Foreground sessions never overlap each other, but merging is still the
    // honest way to get contiguous timeline bands and the active total.
    const presenceRaw: Interval[] = [
      ...rows.map(r => ({ started_at: r.started_at as string, ended_at: r.ended_at as string })),
      ...(active ? [{ started_at: active.started_at, ended_at: nowIso }] : []),
    ]
    const presence_segments = mergeIntervals(presenceRaw)
    const focus_s = unionSeconds(presence_segments)

    // ── Agent overlay (the coding-agent stream, an OVERLAY on top of presence) ─
    // Cap each transcript to its engaged `duration_s` (anchored at its start) so
    // a parked-open Claude window doesn't masquerade as activity. The capped
    // intervals are then split against presence: time spent alongside you is
    // "supervised" (AI-assisted, a subset of focus); time while you were away is
    // "autonomous". Autonomous is NEVER added to focus — it's its own track.
    const agentRaw: Interval[] = codingRows.map(r => ({
      started_at: r.started_at as string,
      ended_at: addSeconds(r.started_at as string, (r.duration_s as number) ?? 0),
    }))
    const agent_segments = mergeIntervals(agentRaw)
    const agent_s = unionSeconds(agent_segments)
    const supervised_s = intersectSeconds(agent_segments, presence_segments)
    const autonomous_s = Math.max(0, agent_s - supervised_s)

    const idle_s = gaps.filter(g => g.kind === 'user_idle').reduce((a, g) => a + g.dur, 0)
    const switch_count = countSwitches(
      sessions.map(s => ({ app: s.app, started_at: s.started_at, dur: s.dur })),
      SWITCH_MIN_DURATION_S,
    )

    const resp: TodayResponse = {
      date,
      sessions,
      active,
      gaps,
      focus_s,
      idle_s,
      agent_s,
      supervised_s,
      autonomous_s,
      presence_segments,
      agent_segments,
      session_count: sessions.length + (active ? 1 : 0),
      switch_count,
    }

    return NextResponse.json(resp)
  } catch (e) {
    console.error('today api error:', e)
    return NextResponse.json({
      date, sessions: [], active: null, gaps: [],
      focus_s: 0, idle_s: 0, agent_s: 0, supervised_s: 0, autonomous_s: 0,
      presence_segments: [], agent_segments: [],
      session_count: 0, switch_count: 0,
    } satisfies TodayResponse)
  }
}
