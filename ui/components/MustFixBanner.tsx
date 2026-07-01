//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// Global must-fix banner. Some tickets are missing the fields Meridian needs to
// track them at all (due date / description / clear title). Those can't be
// ignored, so we surface them at the top of the app (next to the health
// banner) with a one-click way to open the Cleanup modal. Self-hides when
// there are none, or while the Cleanup modal is already open.

import { useEffect, useState } from 'react'
import { hasMustFix } from '@/lib/hygiene'
import type { TasksResponse, IntegrationsResponse } from '@/lib/api-types'
import { load as loadData } from '@/lib/bridge'
import { filterByConnectedProviders } from '@/lib/integrations'

const POLL_MS = 60_000

export default function MustFixBanner({
  onOpenCleanup, hidden,
}: {
  onOpenCleanup: () => void
  hidden?: boolean
}) {
  const [count, setCount] = useState(0)

  useEffect(() => {
    let alive = true
    const load = () => {
      // Fetch independently: a get_integrations failure must not suppress the
      // must-fix count (the shared-catch pattern hides it silently).
      Promise.all([
        loadData<TasksResponse>('/api/tasks', 'get_tasks').catch(() => null),
        loadData<IntegrationsResponse>('/api/integrations', 'get_integrations').catch(() => null),
      ]).then(([taskRes, intRes]) => {
        if (!alive || !taskRes) return // tasks unavailable — don't reset count
        const n = filterByConnectedProviders(taskRes.tasks ?? [], intRes)
          .filter(t => t.hygiene && hasMustFix(t.hygiene.issues)).length
        setCount(n)
      })
    }
    load()
    const timer = setInterval(load, POLL_MS)
    return () => { alive = false; clearInterval(timer) }
  }, [])

  // Don't nag while the Cleanup modal is already open — that's where you fix them.
  if (count === 0 || hidden) return null

  return (
    <button
      onClick={onOpenCleanup}
      className="w-full px-4 py-3 flex items-center justify-between border-b transition-colors text-left"
      style={{ borderBottomColor: 'var(--rule)', backgroundColor: 'var(--warn)' + '14' }}
    >
      <div className="flex items-center gap-3 flex-1 min-w-0">
        <span className="text-lg" style={{ color: 'var(--warn)' }}>⚠️</span>
        <div className="flex-1 min-w-0">
          <p className="text-sm" style={{ color: 'var(--ink-2)' }}>
            <strong>{count} ticket{count === 1 ? '' : 's'} need must-have info</strong>
          </p>
          <p className="text-xs mt-0.5" style={{ color: 'var(--ink-3)' }}>
            Missing a due date, description, or clear title — Meridian can&apos;t track them accurately until these are set.
          </p>
        </div>
      </div>
      <span
        className="px-3 py-1 text-xs rounded shrink-0"
        style={{ background: 'var(--warn)', color: '#fff' }}
      >
        Clean up →
      </span>
    </button>
  )
}
