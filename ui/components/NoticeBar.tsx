//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Global fault banner. Opens one SSE connection to /api/notices/stream and
// renders a banner for each active system notice. Banners auto-disappear when
// the daemon clears the fault — no manual dismiss needed. Placed in the root
// layout so it appears on every page.

'use client'

import { useEffect, useRef, useState } from 'react'
import type { Notice } from '@/lib/notices-store'

const SEVERITY_STYLES: Record<string, { bg: string; border: string; text: string; dot: string }> = {
  error: {
    bg: '#fff5f5',
    border: '#feb2b2',
    text: '#c53030',
    dot: '#e53e3e',
  },
  warning: {
    bg: '#fffbeb',
    border: '#fcd34d',
    text: '#92400e',
    dot: '#d97706',
  },
}

export default function NoticeBar() {
  const [notices, setNotices] = useState<Notice[]>([])
  const esRef = useRef<EventSource | null>(null)

  useEffect(() => {
    function connect() {
      const es = new EventSource('/api/notices/stream')
      esRef.current = es

      es.onmessage = (e) => {
        try {
          setNotices(JSON.parse(e.data) as Notice[])
        } catch {
          // malformed frame — ignore
        }
      }

      es.onerror = () => {
        // Connection dropped — close and reconnect after 5s
        es.close()
        esRef.current = null
        setTimeout(connect, 5_000)
      }
    }

    connect()
    return () => {
      esRef.current?.close()
    }
  }, [])

  if (notices.length === 0) return null

  return (
    <div style={{ position: 'sticky', top: 0, zIndex: 50 }}>
      {notices.map((n) => {
        const s = SEVERITY_STYLES[n.severity] ?? SEVERITY_STYLES.error
        return (
          <div
            key={n.notice_id}
            style={{
              background: s.bg,
              borderBottom: `1px solid ${s.border}`,
              padding: '10px 16px',
              display: 'flex',
              alignItems: 'flex-start',
              gap: 10,
            }}
          >
            <span
              style={{
                display: 'inline-block',
                width: 7,
                height: 7,
                borderRadius: '50%',
                background: s.dot,
                flexShrink: 0,
                marginTop: 5,
              }}
            />
            <div style={{ flex: 1, minWidth: 0 }}>
              <span style={{ fontSize: 13, fontWeight: 600, color: s.text }}>
                {n.title}
              </span>
              <span style={{ fontSize: 12, color: s.text, marginLeft: 8, opacity: 0.85 }}>
                {n.detail}
              </span>
              {n.remedy && (
                <div style={{ marginTop: 2, fontSize: 11, color: s.text, opacity: 0.7 }}>
                  Fix: <code style={{ fontFamily: 'var(--font-geist-mono)', background: 'rgba(0,0,0,0.06)', padding: '1px 4px', borderRadius: 3 }}>{n.remedy}</code>
                </div>
              )}
            </div>
            {n.notice_id.startsWith('pm.') && (
              <a
                href="/tasks?integrations=1"
                style={{
                  flexShrink: 0,
                  fontSize: 11,
                  fontWeight: 600,
                  color: s.text,
                  background: 'rgba(0,0,0,0.07)',
                  border: `1px solid ${s.border}`,
                  borderRadius: 5,
                  padding: '3px 8px',
                  textDecoration: 'none',
                  whiteSpace: 'nowrap',
                  alignSelf: 'center',
                }}
              >
                Fix in Tasks →
              </a>
            )}
          </div>
        )
      })}
    </div>
  )
}
