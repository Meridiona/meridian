//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Global fault banner. Subscribes to the `notices-update` Tauri event (via
// bridge.subscribe) and renders a banner for each active system notice. Banners auto-disappear when
// the daemon clears the fault — no manual dismiss needed. Placed in the root
// layout so it appears on every page.

'use client'

import { useEffect, useState } from 'react'
import { load, subscribe } from '@/lib/bridge'
import type { Notice, RepairPreview } from '@/lib/api-types'

// The one notice that can be acted on in-app rather than in a terminal.
const DB_CORRUPT = 'db.corrupt'

// Palette lives in globals.css (--status-*).
const SEVERITY_STYLES: Record<string, { bg: string; border: string; text: string; dot: string }> = {
  error: {
    bg: 'var(--status-error-bg)',
    border: 'var(--status-error-border)',
    text: 'var(--status-error-text)',
    dot: 'var(--status-error-dot)',
  },
  warning: {
    bg: 'var(--status-warning-bg)',
    border: 'var(--status-warning-border)',
    text: 'var(--status-warning-text)',
    dot: 'var(--status-warning-dot)',
  },
}

export default function NoticeBar() {
  const [notices, setNotices] = useState<Notice[]>([])

  useEffect(() => {
    // notices-update (Tauri event) in the app, /api/notices/stream SSE in a browser.
    return subscribe<Notice[]>('/api/notices/stream', 'get_notices', 'notices-update', setNotices)
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
              {n.remedy && n.notice_id !== DB_CORRUPT && (
                <div style={{ marginTop: 2, fontSize: 11, color: s.text, opacity: 0.7 }}>
                  Fix: <code style={{ fontFamily: 'var(--font-mono)', background: 'rgba(0,0,0,0.06)', padding: '1px 4px', borderRadius: 3 }}>{n.remedy}</code>
                </div>
              )}
            </div>
            {n.notice_id === DB_CORRUPT && <RepairButton color={s.text} border={s.border} />}
            {n.notice_id.startsWith('pm.') && (
              <button
                onClick={() => window.dispatchEvent(new CustomEvent('meridian:open-tasks'))}
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
              </button>
            )}
          </div>
        )
      })}
    </div>
  )
}

// Offers the in-app repair for a damaged database.
//
// Two things this deliberately does NOT do:
//   - Repair silently. The operation loses rows by design, so the user is told
//     what it will cost (from `preview_repair`, a read-only scan) and agrees
//     first - the same consent shape as the DMG updater.
//   - Repair in place. `request_repair` marks the database and RESTARTS the
//     app; the rebuild happens on the way back up, when nothing holds the file
//     open. The button says so, because an app that vanishes without warning
//     reads as a crash.
function RepairButton({ color, border }: { color: string; border: string }) {
  const [busy, setBusy] = useState(false)

  async function onClick() {
    setBusy(true)
    try {
      // Ask what is actually damaged before quoting a cost. A failed preview is
      // not a reason to block the repair - the database is already broken - so
      // fall back to generic wording rather than refusing to proceed.
      let cost = 'Some recent activity data may be lost.'
      try {
        const p = await load<RepairPreview>('/api/db/repair/preview', 'preview_repair')
        if (p.product_tables.length > 0) {
          cost = `Damage reaches ${p.product_tables.length} table(s) holding your data - some records cannot be recovered.`
        } else if (p.corrupt_tables.length > 0) {
          cost = 'Only recent screen-activity data is damaged. Your history is intact.'
        }
      } catch {
        // keep the generic wording
      }

      const ok = window.confirm(
        `Repair Meridian's database?\n\n${cost}\n\nEverything readable is kept, and the damaged copy is saved alongside it. Meridian will restart to do this.`,
      )
      if (!ok) return
      await load('/api/db/repair', 'request_repair')
    } catch (e) {
      window.alert(`Could not start the repair: ${e instanceof Error ? e.message : String(e)}`)
    } finally {
      setBusy(false)
    }
  }

  return (
    <button
      onClick={onClick}
      disabled={busy}
      style={{
        flexShrink: 0,
        fontSize: 11,
        fontWeight: 600,
        color,
        background: 'rgba(0,0,0,0.07)',
        border: `1px solid ${border}`,
        borderRadius: 5,
        padding: '3px 8px',
        whiteSpace: 'nowrap',
        alignSelf: 'center',
        cursor: busy ? 'default' : 'pointer',
        opacity: busy ? 0.6 : 1,
      }}
    >
      {busy ? 'Starting…' : 'Repair Database'}
    </button>
  )
}
