//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Horizontal "time by app" bar chart, shared by the Overview panel (whole day)
// and the Hour-detail panel (one hour). Aggregates get_today sessions by app —
// the sessions carry app + dur, so this buckets client-side. Bar hue is
// deterministic per app name (same scheme as atoms' AppGlyph), keeping colors
// stable across renders without depending on a category.

'use client'

import { useMemo } from 'react'
import { fmtDur, AppGlyph } from '@/components/atoms'
import type { TodaySession } from '@/lib/api-types'

function appHue(app: string): number {
  let h = 0
  for (let i = 0; i < app.length; i++) h = (h * 31 + app.charCodeAt(i)) & 0xffff
  return h % 360
}

/** Aggregate sessions into app totals, descending, capped to `limit`. */
export function appTotals(sessions: TodaySession[]): Array<{ app: string; seconds: number }> {
  const by = new Map<string, number>()
  for (const s of sessions) {
    if (!s.app) continue
    by.set(s.app, (by.get(s.app) ?? 0) + s.dur)
  }
  return Array.from(by.entries())
    .map(([app, seconds]) => ({ app, seconds }))
    .sort((a, b) => b.seconds - a.seconds)
}

export function TimeByApp({ sessions, limit = 6 }: { sessions: TodaySession[]; limit?: number }) {
  const rows = useMemo(() => appTotals(sessions).slice(0, limit), [sessions, limit])
  const max = rows[0]?.seconds ?? 1

  if (rows.length === 0) {
    return <p className="mt-body-sm italic" style={{ color: 'var(--t-faint-2)' }}>No app activity yet.</p>
  }

  return (
    <div className="space-y-2">
      {rows.map(({ app, seconds }) => (
        <div key={app} className="flex items-center gap-2.5">
          <AppGlyph app={app} size={18} />
          <span className="mt-body-sm truncate w-24 shrink-0" style={{ color: 'var(--t-muted)' }}>{app}</span>
          <span className="flex-1 h-2 rounded-full overflow-hidden bg-track">
            <span className="block h-full rounded-full" style={{
              width: `${Math.max(4, (seconds / max) * 100)}%`,
              background: `hsl(${appHue(app)}, 55%, 55%)`,
            }} />
          </span>
          <span className="mt-mono-sm text-[11px] shrink-0" style={{ color: 'var(--t-faint)' }}>{fmtDur(seconds)}</span>
        </div>
      ))}
    </div>
  )
}
