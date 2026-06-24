//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// In-app notification banner — the banner-channel half of the notification
// outbox (the redundant counterpart to the tray's macOS toast). Subscribes to
// the `notifications-update` Tauri event (via bridge.subscribe) and renders a
// dismissible banner per active notification. Distinct from NoticeBar (stateful faults, auto-clear):
// these are discrete events the user dismisses, or that expire on their own.

'use client'

import { useEffect, useState } from 'react'
import type { BannerNotification } from '@/lib/api-types'
import { invoke, subscribe } from '@/lib/bridge'

const SEVERITY_STYLES: Record<string, { bg: string; border: string; text: string; dot: string }> = {
  info:    { bg: '#eff6ff', border: '#bfdbfe', text: '#1d4ed8', dot: '#2563eb' },
  warning: { bg: '#fffbeb', border: '#fcd34d', text: '#92400e', dot: '#d97706' },
  error:   { bg: '#fff5f5', border: '#feb2b2', text: '#c53030', dot: '#e53e3e' },
}

export default function NotificationBanner() {
  const [items, setItems] = useState<BannerNotification[]>([])

  useEffect(() => {
    // notifications-update (Tauri event) in the app, /api/notifications/stream
    // SSE in a browser. subscribe() owns the reconnect/teardown.
    return subscribe<BannerNotification[]>(
      '/api/notifications/stream',
      'get_banner_notifications',
      'notifications-update',
      setItems,
    )
  }, [])

  async function dismiss(id: number) {
    // Optimistic remove; the next notifications-update reconciles.
    setItems(prev => prev.filter(i => i.id !== id))
    try {
      await invoke('dismiss_notification', { id })
    } catch { /* ignore — optimistic UI already updated */ }
  }

  if (items.length === 0) return null

  return (
    <div style={{ position: 'sticky', top: 0, zIndex: 49 }}>
      {items.map((n) => {
        const s = SEVERITY_STYLES[n.severity] ?? SEVERITY_STYLES.info
        return (
          <div key={n.id}
            style={{ background: s.bg, borderBottom: `1px solid ${s.border}`, padding: '10px 16px', display: 'flex', alignItems: 'flex-start', gap: 10 }}>
            <span style={{ display: 'inline-block', width: 7, height: 7, borderRadius: '50%', background: s.dot, flexShrink: 0, marginTop: 5 }} />
            <div style={{ flex: 1, minWidth: 0 }}>
              <span style={{ fontSize: 13, fontWeight: 600, color: s.text }}>{n.title}</span>
              {n.body && <span style={{ fontSize: 12, color: s.text, marginLeft: 8, opacity: 0.85 }}>{n.body}</span>}
            </div>
            {n.deep_link && (
              <a href={n.deep_link}
                style={{ flexShrink: 0, fontSize: 11, fontWeight: 600, color: s.text, background: 'rgba(0,0,0,0.07)', border: `1px solid ${s.border}`, borderRadius: 5, padding: '3px 8px', textDecoration: 'none', whiteSpace: 'nowrap', alignSelf: 'center' }}>
                Open →
              </a>
            )}
            <button onClick={() => dismiss(n.id)} aria-label="Dismiss"
              style={{ flexShrink: 0, fontSize: 15, lineHeight: 1, color: s.text, background: 'transparent', border: 'none', cursor: 'pointer', opacity: 0.6, alignSelf: 'center', padding: '0 2px' }}>
              ×
            </button>
          </div>
        )
      })}
    </div>
  )
}
