// meridian — AI activity intelligence by Meridiona

import { notFound } from 'next/navigation'
import Link from 'next/link'
import { ArrowLeft, ExternalLink, Mic, Clipboard, ArrowRight } from 'lucide-react'
import getDb from '@/lib/db'
import AppIcon from '@/components/AppIcon'
import CategoryBadge from '@/components/CategoryBadge'
import TaskBadge from '@/components/TaskBadge'
import { formatDuration, formatTime } from '@/lib/format'
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

interface PageData {
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

function loadSession(id: number): PageData | null {
  try {
    const db = getDb()
    const sessionRow = db.prepare(`
      SELECT s.id, s.app_name, s.started_at, s.ended_at, s.duration_s,
             s.window_titles, s.audio_snippets, s.signals, s.frame_count, s.etl_run_id,
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
       WHERE s.id = ?
    `).get(id) as Record<string, unknown> | undefined
    if (!sessionRow) return null

    let dimensions: DimensionRow[] = []
    try {
      dimensions = db.prepare(`
        SELECT dimension, value, confidence, source, created_at
          FROM session_dimensions
         WHERE session_id = ?
         ORDER BY dimension, confidence DESC
      `).all(id) as DimensionRow[]
    } catch {}

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
    } catch {}

    let summary_json: Record<string, unknown> | null = null
    try {
      const sumRow = db.prepare(`
        SELECT summary_json FROM session_summaries WHERE session_id = ?
      `).get(id) as { summary_json: string } | undefined
      if (sumRow?.summary_json) {
        try { summary_json = JSON.parse(sumRow.summary_json) } catch {}
      }
    } catch {}

    return {
      session: parseRow(sessionRow),
      dimensions,
      dispatches,
      summary_json,
    }
  } catch (e) {
    console.error('loadSession error', e)
    return null
  }
}

export default async function SessionDetailPage({
  params,
}: {
  params: Promise<{ id: string }>
}) {
  const { id: idStr } = await params
  const id = Number(idStr)
  if (!Number.isFinite(id) || id <= 0) notFound()
  const data = loadSession(id)
  if (!data) notFound()

  const { session, dimensions, dispatches, summary_json } = data!

  const dimsByGroup = dimensions.reduce<Record<string, DimensionRow[]>>((acc, d) => {
    ;(acc[d.dimension] ??= []).push(d)
    return acc
  }, {})
  const dimOrder = [
    'activity', 'intent', 'engagement', 'collaboration',
    'tool', 'topic', 'practice',
  ]

  return (
    <div className="space-y-6">
      <Link
        href="/sessions"
        className="inline-flex items-center gap-1 text-xs text-[#9B9A97] hover:text-[#6B6A67] transition-colors"
      >
        <ArrowLeft className="w-3.5 h-3.5" />
        all sessions
      </Link>

      {/* Header */}
      <div className="rounded-2xl border border-[#E8E6E1] bg-white p-5">
        <div className="flex items-start gap-3">
          <AppIcon appName={session.app_name} size="md" />
          <div className="flex-1 min-w-0">
            <div className="flex items-center gap-2">
              <h1 className="text-lg font-semibold text-[#141414]">{session.app_name}</h1>
              <CategoryBadge category={session.category} confidence={session.confidence} size="sm" />
              <TaskBadge
                taskKey={session.task_key}
                sessionType={session.session_type}
                routing={session.routing}
                confidence={session.link_confidence}
                method={session.link_method}
                taskTitle={session.task_title}
                taskUrl={session.task_url}
                size="sm"
              />
            </div>
            <p className="text-xs text-[#9B9A97] mt-1 font-mono tabular-nums">
              {formatTime(session.started_at)} → {formatTime(session.ended_at)}
              <span className="ml-2">· {formatDuration(session.duration_s)}</span>
              <span className="ml-2">· session #{session.id}</span>
            </p>
          </div>
        </div>

        {typeof summary_json?.summary === 'string' && summary_json.summary.length > 0 ? (
          <p className="text-sm text-[#6B6A67] mt-4 leading-relaxed">
            {summary_json.summary}
          </p>
        ) : null}
      </div>

      {/* Classification */}
      {session.session_type && (
        <section>
          <p className="text-[10px] uppercase tracking-widest text-[#C8C6C1] mb-3">Classification</p>
          <div className="rounded-2xl border border-[#E8E6E1] bg-white p-5 grid grid-cols-2 sm:grid-cols-4 gap-y-3 gap-x-6 text-xs">
            <Field label="task_key">
              {session.task_key ? (
                session.task_url ? (
                  <a href={session.task_url} target="_blank" rel="noopener noreferrer"
                     className="font-mono text-[#3D5BB0] hover:underline inline-flex items-center gap-1">
                    {session.task_key}
                    <ExternalLink className="w-2.5 h-2.5" />
                  </a>
                ) : (
                  <span className="font-mono text-[#3D5BB0]">{session.task_key}</span>
                )
              ) : (
                <span className="text-[#9B9A97]">—</span>
              )}
            </Field>
            <Field label="session_type">
              <span className="text-[#6B6A67]">{session.session_type}</span>
            </Field>
            <Field label="routing">
              <span className="text-[#6B6A67]">{session.routing ?? '—'}</span>
            </Field>
            <Field label="confidence">
              <span className="font-mono text-[#6B6A67] tabular-nums">
                {typeof session.link_confidence === 'number' ? session.link_confidence.toFixed(2) : '—'}
              </span>
            </Field>
            <Field label="method">
              <span className="font-mono text-[#9B9A97]">{session.link_method ?? '—'}</span>
            </Field>
            {session.task_title && (
              <div className="col-span-2 sm:col-span-3">
                <p className="text-[10px] uppercase tracking-widest text-[#C8C6C1] mb-1">title</p>
                <p className="text-xs text-[#6B6A67]">{session.task_title}</p>
              </div>
            )}
          </div>
        </section>
      )}

      {/* Dimensions */}
      {dimensions.length > 0 && (
        <section>
          <p className="text-[10px] uppercase tracking-widest text-[#C8C6C1] mb-3">Dimensions</p>
          <div className="rounded-2xl border border-[#E8E6E1] bg-white p-5 space-y-3">
            {dimOrder
              .filter(g => dimsByGroup[g] && dimsByGroup[g].length > 0)
              .map(g => (
                <div key={g} className="flex items-baseline gap-3 text-xs">
                  <span className="w-28 shrink-0 uppercase tracking-widest text-[10px] text-[#C8C6C1]">{g}</span>
                  <div className="flex-1 flex flex-wrap gap-1.5">
                    {dimsByGroup[g].map((d, i) => (
                      <span key={`${d.value}-${i}`} className="inline-flex items-center gap-1 bg-[#F8F7F4] text-[#6B6A67] rounded px-2 py-0.5">
                        <span>{d.value}</span>
                        <span className="font-mono text-[10px] text-[#9B9A97]">
                          {Math.round(d.confidence * 100)}%
                        </span>
                      </span>
                    ))}
                  </div>
                </div>
              ))}
          </div>
        </section>
      )}

      {/* Window Titles */}
      {session.window_titles.length > 0 && (
        <section>
          <p className="text-[10px] uppercase tracking-widest text-[#C8C6C1] mb-3">Window Titles</p>
          <div className="rounded-2xl border border-[#E8E6E1] bg-white p-5">
            <ul className="space-y-1.5 text-xs">
              {session.window_titles.slice(0, 30).map((w) => (
                <li key={w.window_name} className="flex items-baseline justify-between gap-2">
                  <span className="text-[#6B6A67] truncate">{w.window_name}</span>
                  {w.count > 1 && (
                    <span className="font-mono text-[10px] text-[#9B9A97] tabular-nums shrink-0">
                      ×{w.count}
                    </span>
                  )}
                </li>
              ))}
            </ul>
          </div>
        </section>
      )}

      {/* Audio */}
      {session.audio_snippets && session.audio_snippets.length > 0 && (
        <section>
          <p className="text-[10px] uppercase tracking-widest text-[#C8C6C1] mb-3">Audio</p>
          <div className="rounded-2xl border border-[#E8E6E1] bg-white p-5 space-y-2">
            {session.audio_snippets.slice(0, 20).map((a, i) => (
              <div key={i} className="flex items-start gap-2">
                <Mic className="w-3 h-3 text-[#C8C6C1] mt-0.5 shrink-0" />
                <p className="text-xs text-[#6B6A67] leading-relaxed">{a.transcription}</p>
              </div>
            ))}
          </div>
        </section>
      )}

      {/* Signals */}
      {session.signals && session.signals.length > 0 && (
        <section>
          <p className="text-[10px] uppercase tracking-widest text-[#C8C6C1] mb-3">Signals</p>
          <div className="rounded-2xl border border-[#E8E6E1] bg-white p-5 space-y-1.5">
            {session.signals.slice(0, 20).map((s, i) => (
              <div key={i} className="flex items-start gap-2">
                {s.event_type === 'clipboard' ? (
                  <Clipboard className="w-3 h-3 text-[#C8C6C1] mt-0.5 shrink-0" />
                ) : (
                  <ArrowRight className="w-3 h-3 text-[#C8C6C1] mt-0.5 shrink-0" />
                )}
                <p className="text-xs text-[#6B6A67] font-mono truncate">{s.value}</p>
              </div>
            ))}
          </div>
        </section>
      )}

      {/* Dispatch queue */}
      {dispatches.length > 0 && (
        <section>
          <p className="text-[10px] uppercase tracking-widest text-[#C8C6C1] mb-3">Dispatch Queue</p>
          <div className="rounded-2xl border border-[#E8E6E1] bg-white overflow-hidden">
            <table className="w-full text-xs">
              <thead>
                <tr className="text-[10px] uppercase tracking-widest text-[#C8C6C1]">
                  <th className="text-left px-4 py-2 font-medium">id</th>
                  <th className="text-left px-4 py-2 font-medium">task</th>
                  <th className="text-left px-4 py-2 font-medium">provider</th>
                  <th className="text-left px-4 py-2 font-medium">state</th>
                  <th className="text-left px-4 py-2 font-medium">created</th>
                </tr>
              </thead>
              <tbody>
                {dispatches.map((d) => (
                  <tr key={d.id} className="border-t border-[#F0EFEC]">
                    <td className="px-4 py-2 font-mono text-[#9B9A97]">#{d.id}</td>
                    <td className="px-4 py-2 font-mono text-[#3D5BB0]">{d.task_key}</td>
                    <td className="px-4 py-2 text-[#6B6A67]">{d.provider}</td>
                    <td className="px-4 py-2">
                      <DispatchStatePill state={d.state} attempts={d.attempts} />
                    </td>
                    <td className="px-4 py-2 font-mono text-[#9B9A97] tabular-nums">{d.created_at}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </section>
      )}
    </div>
  )
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div>
      <p className="text-[10px] uppercase tracking-widest text-[#C8C6C1] mb-1">{label}</p>
      <p>{children}</p>
    </div>
  )
}

function DispatchStatePill({ state, attempts }: { state: string; attempts: number }) {
  const palette = state === 'sent'
    ? 'bg-[#EBF8F0] text-[#4A9E6A]'
    : state === 'failed'
      ? 'bg-[#FBEBEB] text-[#C44A4A]'
      : state === 'skipped'
        ? 'bg-[#F0EFEC] text-[#9B9A97]'
        : 'bg-[#FBF6EB] text-[#C49E4A]'   // pending
  return (
    <span className={`inline-flex items-center gap-1 rounded-full px-2 py-0.5 font-medium ${palette}`}>
      {state}
      {attempts > 0 && state !== 'sent' && (
        <span className="font-mono opacity-60">×{attempts}</span>
      )}
    </span>
  )
}
