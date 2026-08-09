//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Global fault banner. Subscribes to the `notices-update` Tauri event (via
// bridge.subscribe) and renders a banner for each active system notice. Placed
// in the root layout so it appears on every page.
//
// Most banners auto-disappear when the daemon clears the fault, and those have
// no dismiss control on purpose — hiding a live fault would only make it
// invisible, not fixed. The exception is ACKNOWLEDGEABLE (below): notices that
// report a past EVENT rather than a current condition. Nothing can ever
// "recover" from them, so without a dismiss they sit on screen forever.

'use client'

import { useEffect, useState } from 'react'
import { load, mutate, subscribe } from '@/lib/bridge'
import ConfirmDialog from '@/components/ConfirmDialog'
import { formatSince } from '@/lib/notice-time'
import type { Notice, RepairPreview } from '@/lib/api-types'

// The one notice that can be acted on in-app rather than in a terminal.
const DB_CORRUPT = 'db.corrupt'

// Notices the user dismisses, because nothing else will.
//
// Both are raised once by the tray's boot-time repair (`lib.rs`) to report what
// it did to the database. They describe something that already happened, so no
// code path clears them — and NoticeBar had no dismiss — which meant any
// machine that ever ran a boot repair carried the banner permanently. Same
// shape as the 22 undismissable board-hygiene banners: a producer with no
// matching consumer.
//
// A live fault must NOT be added here. If a notice can clear itself when the
// condition passes, let it.
const ACKNOWLEDGEABLE = new Set(['db.repaired', 'db.reset'])

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
        // When the fault was raised. A notice can outlive its cause (the
        // db.corrupt banner once survived its own repair), and the stamp is
        // what lets a user tell a stale alarm from a live one.
        const since = formatSince(n.raised_at)
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
              {since && (
                <span style={{ fontSize: 11, color: s.text, marginLeft: 8, opacity: 0.6, whiteSpace: 'nowrap' }}>
                  {since}
                </span>
              )}
              {n.remedy && n.notice_id !== DB_CORRUPT && (
                <div style={{ marginTop: 2, fontSize: 11, color: s.text, opacity: 0.7 }}>
                  Fix: <code style={{ fontFamily: 'var(--font-mono)', background: 'rgba(0,0,0,0.06)', padding: '1px 4px', borderRadius: 3 }}>{n.remedy}</code>
                </div>
              )}
            </div>
            {n.notice_id === DB_CORRUPT && <RepairButton color={s.text} border={s.border} />}
            {/* A tracker fault is fixed by reconnecting the tracker. This used
                to say "Fix in Tasks →" and open the Tasks modal, which
                disagreed with both this notice's own remedy text ("Reconnect X
                in Settings → Integrations") and its toast's click-through -
                three surfaces for one fault, two destinations. */}
            {n.notice_id.startsWith('pm.') && (
              <button
                onClick={() => window.dispatchEvent(
                  new CustomEvent('meridian:open-settings', { detail: 'integrations' })
                )}
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
                  cursor: 'pointer',
                }}
              >
                Reconnect →
              </button>
            )}
            {ACKNOWLEDGEABLE.has(n.notice_id) && (
              <DismissButton noticeId={n.notice_id} color={s.text}
                onDone={() => setNotices(cur => cur.filter(x => x.notice_id !== n.notice_id))} />
            )}
          </div>
        )
      })}
    </div>
  )
}

// Acknowledge a notice that reports a past event (see ACKNOWLEDGEABLE).
//
// Optimistic: the row is dropped locally the moment the write succeeds rather
// than waiting for the next `notices-update` push, so the banner does not
// linger for a poll interval after the click. A failed write leaves the banner
// up - which is the honest outcome, since the notice really is still there.
function DismissButton({ noticeId, color, onDone }: {
  noticeId: string
  color: string
  onDone: () => void
}) {
  const [busy, setBusy] = useState(false)
  return (
    <button
      aria-label="Dismiss"
      title="Dismiss"
      disabled={busy}
      onClick={async () => {
        setBusy(true)
        try {
          await mutate('/api/notices', 'delete_notice', { noticeId }, 'DELETE')
          onDone()
        } catch {
          setBusy(false)
        }
      }}
      style={{
        flexShrink: 0,
        alignSelf: 'center',
        background: 'transparent',
        border: 'none',
        color,
        opacity: busy ? 0.4 : 0.6,
        fontSize: 14,
        lineHeight: 1,
        padding: '2px 6px',
        cursor: busy ? 'default' : 'pointer',
      }}
    >
      ✕
    </button>
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
//
// The consent gate is `ConfirmDialog`, NOT `window.confirm` - that global
// returns false in the packaged tray, which made this button a no-op from the
// day it shipped (see ConfirmDialog's header). Errors render in the dialog for
// the same reason: `window.alert` never appeared either, so a failed repair and
// a dismissed one looked identical.
function RepairButton({ color, border }: { color: string; border: string }) {
  // `previewing` is the click-to-dialog gap; `starting` is request_repair in
  // flight, which normally ends with the app restarting rather than a result.
  const [stage, setStage] = useState<'idle' | 'previewing' | 'confirming' | 'starting'>('idle')
  const [cost, setCost] = useState('')
  const [error, setError] = useState('')

  const busy = stage === 'previewing' || stage === 'starting'

  async function onClick() {
    setStage('previewing')
    setError('')
    // Ask what is actually damaged before quoting a cost. A failed preview is
    // not a reason to block the repair - the database is already broken - so
    // fall back to generic wording rather than refusing to proceed.
    let quoted = 'Some recent activity data may be lost.'
    try {
      const p = await load<RepairPreview>('/api/db/repair/preview', 'preview_repair')
      if (p.product_tables.length > 0) {
        quoted = `Damage reaches ${p.product_tables.length} table(s) holding your data - some records cannot be recovered.`
      } else if (p.corrupt_tables.length > 0) {
        quoted = 'Only recent screen-activity data is damaged. Your history is intact.'
      }
    } catch {
      // keep the generic wording
    }
    setCost(quoted)
    setStage('confirming')
  }

  async function onConfirm() {
    setStage('starting')
    setError('')
    try {
      await load('/api/db/repair', 'request_repair')
      // Not reached in practice - the command relaunches the app. Only a
      // failure returns here, so leave the dialog up rather than closing it.
    } catch (e) {
      setError(`Could not start the repair: ${e instanceof Error ? e.message : String(e)}`)
      setStage('confirming')
    }
  }

  return (
    <>
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
      {(stage === 'confirming' || stage === 'starting') && (
        <ConfirmDialog
          title="Repair Meridian's database?"
          body={`${cost}\n\nEverything readable is kept, and the damaged copy is saved alongside it. Meridian will restart to do this.`}
          confirmLabel="Repair and restart"
          busyLabel="Starting…"
          busy={stage === 'starting'}
          error={error}
          onConfirm={onConfirm}
          onCancel={() => { setStage('idle'); setError('') }}
        />
      )}
    </>
  )
}
