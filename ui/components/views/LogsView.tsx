//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Live log viewer. Loads recent entries on mount, then tails new ones via SSE.
// Filterable by level. Auto-scrolls to bottom unless the user has scrolled up.

'use client'

import { useEffect, useRef, useState, useCallback } from 'react'
import type { LogEntry } from '@/lib/log-tail'
import { load, subscribe } from '@/lib/bridge'

const LEVEL_STYLES: Record<string, { bg: string; color: string; label: string }> = {
  ERROR: { bg: '#fee2e2', color: '#b91c1c', label: 'ERR' },
  WARN:  { bg: '#fef3c7', color: '#92400e', label: 'WRN' },
  INFO:  { bg: '#dbeafe', color: '#1e40af', label: 'INF' },
  DEBUG: { bg: '#f3f4f6', color: '#6b7280', label: 'DBG' },
  TRACE: { bg: '#f3f4f6', color: '#9ca3af', label: 'TRC' },
}

const FILTERS = ['ALL', 'ERROR', 'WARN', 'INFO', 'DEBUG'] as const
type Filter = (typeof FILTERS)[number]

function shortTarget(target: string): string {
  const parts = target.split('::')
  return parts.length > 2 ? parts.slice(-2).join('::') : target
}

function relativeTime(iso: string): string {
  const delta = Date.now() - new Date(iso).getTime()
  if (delta < 60_000) return `${Math.floor(delta / 1000)}s ago`
  if (delta < 3_600_000) return `${Math.floor(delta / 60_000)}m ago`
  return new Date(iso).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' })
}

function levelOrder(level: string): number {
  return { ERROR: 0, WARN: 1, INFO: 2, DEBUG: 3, TRACE: 4 }[level] ?? 5
}

export default function LogsView() {
  const [entries, setEntries] = useState<LogEntry[]>([])
  const [filter, setFilter] = useState<Filter>('ALL')
  const [expanded, setExpanded] = useState<Set<number>>(new Set())
  const [paused, setPaused] = useState(false)
  const bottomRef = useRef<HTMLDivElement>(null)
  const containerRef = useRef<HTMLDivElement>(null)
  const pausedRef = useRef(false)
  pausedRef.current = paused

  // Auto-scroll: only when user hasn't scrolled up
  const shouldAutoScroll = useRef(true)
  const onScroll = useCallback(() => {
    const el = containerRef.current
    if (!el) return
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 80
    shouldAutoScroll.current = atBottom
  }, [])

  useEffect(() => {
    // Prime with the last 200 entries (dual-path: get_logs / /api/logs).
    load<LogEntry[]>('/api/logs', 'get_logs', { limit: 200 })
      .then(data => setEntries(data))
      .catch(() => {})

    // Tail new entries: log-tail (Tauri event) in the app, /api/logs/stream SSE
    // in a browser. command=null — the event carries DELTAS (we append), and the
    // prime above already loaded the snapshot, so subscribe must not re-prime.
    return subscribe<LogEntry[]>('/api/logs/stream', null, 'log-tail', (incoming) => {
      if (pausedRef.current || !incoming?.length) return
      setEntries(prev => [...prev, ...incoming].slice(-2000)) // cap at 2000
    })
  }, [])

  useEffect(() => {
    if (shouldAutoScroll.current) {
      bottomRef.current?.scrollIntoView({ behavior: 'smooth' })
    }
  }, [entries])

  const visible = filter === 'ALL'
    ? entries
    : entries.filter(e => levelOrder(e.level) <= levelOrder(filter))

  const toggleExpand = (idx: number) => {
    setExpanded(prev => {
      const next = new Set(prev)
      if (next.has(idx)) next.delete(idx); else next.add(idx)
      return next
    })
  }

  return (
    <div style={{ display: 'flex', flexDirection: 'column', height: '100%', minHeight: 0 }}>
      {/* Toolbar */}
      <div style={{
        display: 'flex', alignItems: 'center', gap: 12,
        padding: '12px 16px', borderBottom: '1px solid var(--rule)', flexShrink: 0,
      }}>
        <p style={{ fontSize: 11, textTransform: 'uppercase', letterSpacing: '0.15em', color: 'var(--ink-3)' }}>
          Daemon Logs
        </p>
        <div style={{ marginLeft: 'auto', display: 'flex', gap: 6, alignItems: 'center' }}>
          {FILTERS.map(f => (
            <button
              key={f}
              onClick={() => setFilter(f)}
              style={{
                fontSize: 11, padding: '2px 8px', borderRadius: 4, cursor: 'pointer',
                border: `1px solid ${filter === f ? 'var(--ink-3)' : 'var(--rule)'}`,
                background: filter === f ? 'var(--ink)' : 'transparent',
                color: filter === f ? 'var(--canvas)' : 'var(--ink-3)',
              }}
            >
              {f}
            </button>
          ))}
          <button
            onClick={() => setPaused(p => !p)}
            style={{
              fontSize: 11, padding: '2px 8px', borderRadius: 4, cursor: 'pointer',
              border: `1px solid ${paused ? '#fcd34d' : 'var(--rule)'}`,
              background: paused ? '#fef3c7' : 'transparent',
              color: paused ? '#92400e' : 'var(--ink-3)',
            }}
          >
            {paused ? '▶ Resume' : '⏸ Pause'}
          </button>
        </div>
      </div>

      {/* Log list */}
      <div
        ref={containerRef}
        onScroll={onScroll}
        style={{
          flex: 1, overflowY: 'auto', fontFamily: 'var(--font-mono)',
          fontSize: 12, lineHeight: 1.6,
        }}
      >
        {visible.length === 0 && (
          <p style={{ padding: 24, color: 'var(--ink-4)', fontSize: 13 }}>
            No log entries — ensure the daemon is running.
          </p>
        )}
        {visible.map((entry, idx) => {
          const style = LEVEL_STYLES[entry.level] ?? LEVEL_STYLES.DEBUG
          const hasFields = Object.keys(entry.fields).length > 0
          const isExpanded = expanded.has(idx)
          return (
            <div
              key={idx}
              onClick={() => hasFields && toggleExpand(idx)}
              style={{
                display: 'flex', gap: 10, padding: '3px 16px',
                borderBottom: '1px solid var(--rule)',
                cursor: hasFields ? 'pointer' : 'default',
                background: isExpanded ? 'var(--canvas-2)' : 'transparent',
              }}
            >
              <span style={{ color: 'var(--ink-4)', flexShrink: 0, width: 60, fontSize: 11 }}>
                {relativeTime(entry.timestamp)}
              </span>
              <span style={{
                flexShrink: 0, width: 28, fontSize: 10, fontWeight: 700,
                padding: '0 3px', borderRadius: 3,
                background: style.bg, color: style.color,
                display: 'inline-flex', alignItems: 'center', justifyContent: 'center',
              }}>
                {style.label}
              </span>
              <span style={{ color: 'var(--ink-4)', flexShrink: 0, width: 180, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', fontSize: 11 }}>
                {shortTarget(entry.target)}
                {entry.span ? ` › ${entry.span}` : ''}
              </span>
              <div style={{ flex: 1, minWidth: 0 }}>
                <span style={{ color: 'var(--ink)', whiteSpace: 'pre-wrap', wordBreak: 'break-word' }}>
                  {entry.message}
                </span>
                {isExpanded && hasFields && (
                  <pre style={{
                    marginTop: 4, padding: '6px 10px', borderRadius: 4,
                    background: 'var(--canvas-3)', color: 'var(--ink-2)',
                    fontSize: 11, whiteSpace: 'pre-wrap', wordBreak: 'break-all',
                  }}>
                    {JSON.stringify(entry.fields, null, 2)}
                  </pre>
                )}
              </div>
            </div>
          )
        })}
        <div ref={bottomRef} />
      </div>
    </div>
  )
}
