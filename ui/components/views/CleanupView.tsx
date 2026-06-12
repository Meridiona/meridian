//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

import { useCallback, useEffect, useMemo, useState } from 'react'
import { TaskKey, ProviderGlyph } from '@/components/atoms'
import HygieneDialog from '@/components/HygieneDialog'
import { hasMustFix } from '@/lib/hygiene'
import type { TaskSummary, TasksResponse } from '@/app/api/tasks/route'

type Section = {
  key: string
  title: string
  blurb: string
  tone: string
  tasks: TaskSummary[]
}

export default function CleanupView() {
  const [tasks, setTasks] = useState<TaskSummary[]>([])
  const [loading, setLoading] = useState(true)
  const [fixTask, setFixTask] = useState<TaskSummary | null>(null)

  const load = useCallback(() => {
    fetch('/api/tasks')
      .then(r => r.json())
      .then((res: TasksResponse) => { setTasks(res.tasks ?? []); setLoading(false) })
      .catch(() => setLoading(false))
  }, [])

  useEffect(() => { load() }, [load])

  const sections = useMemo<Section[]>(() => {
    const flagged = tasks.filter(t => t.hygiene)
    const issues = (t: TaskSummary) => t.hygiene?.issues ?? []
    const mustFix = flagged.filter(t => hasMustFix(issues(t)))
    const niceToFix = flagged.filter(t => !hasMustFix(issues(t)) && issues(t).length > 0)
    const review = flagged.filter(t =>
      issues(t).length === 0 && (t.hygiene!.bucket === 'looks_stale' || t.hygiene!.bucket === 'not_sure'))
    return [
      { key: 'must', title: 'Must fix', blurb: "Meridian can't track these accurately until they're cleared — a due date, a description, or a clearer title.", tone: 'var(--warn)', tasks: mustFix },
      { key: 'nice', title: 'Nice to fix', blurb: 'Good hygiene — an epic, labels, a priority or estimate. Tidy when you have a moment.', tone: 'var(--accent)', tasks: niceToFix },
      { key: 'review', title: 'Review', blurb: 'Looks stale or unclear. Keep it, or close it in your tracker.', tone: 'var(--ink-3)', tasks: review },
    ]
  }, [tasks])

  if (loading) return <div className="p-8 text-sm" style={{ color: 'var(--ink-3)' }}>Reading your board…</div>

  const total = sections.reduce((n, s) => n + s.tasks.length, 0)

  return (
    <div className="max-w-3xl mx-auto p-6 md:p-8">
      <header className="mb-6">
        <h1 className="text-[22px] font-medium mb-1" style={{ color: 'var(--ink)' }}>Board clean-up</h1>
        <p className="text-sm" style={{ color: 'var(--ink-3)' }}>
          {total === 0
            ? 'Your board is clean — nothing needs attention.'
            : `${total} ticket${total === 1 ? '' : 's'} could use a tidy. Must-fix first.`}
        </p>
      </header>

      {sections.map(s => s.tasks.length > 0 && (
        <section key={s.key} className="mb-8">
          <div className="flex items-center gap-2 mb-1">
            <span className="w-2 h-2 rounded-full" style={{ background: s.tone }} />
            <h2 className="text-[15px] font-medium" style={{ color: 'var(--ink)' }}>
              {s.title} <span style={{ color: 'var(--ink-3)' }}>· {s.tasks.length}</span>
            </h2>
          </div>
          <p className="text-[12px] mb-2.5 max-w-xl" style={{ color: 'var(--ink-3)' }}>{s.blurb}</p>
          <div className="space-y-2">
            {s.tasks.map(t => (
              <button key={t.key} onClick={() => setFixTask(t)}
                className="w-full text-left rounded-xl border p-3 flex items-center gap-3 transition-colors"
                style={{ borderColor: 'var(--rule)', background: 'var(--surface)' }}>
                <ProviderGlyph provider={t.provider} size={18} />
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2">
                    <TaskKey keyId={t.key} />
                    <span className="text-[13px] font-medium truncate" style={{ color: 'var(--ink)' }}>{t.title}</span>
                  </div>
                  <p className="text-[11px] mt-1 truncate" style={{ color: 'var(--ink-3)' }}>
                    {(t.hygiene?.issues ?? []).map(i => i.hint).join(' · ') || 'Review this ticket.'}
                  </p>
                </div>
                <span className="text-[12px] px-2.5 py-1 rounded-md shrink-0" style={{ background: s.tone, color: '#fff' }}>
                  {s.key === 'review' ? 'Review' : 'Fix'}
                </span>
              </button>
            ))}
          </div>
        </section>
      ))}

      {total === 0 && (
        <div className="rounded-2xl border p-8 text-center" style={{ background: 'var(--surface)', borderColor: 'var(--rule)' }}>
          <div aria-hidden style={{ fontSize: 32 }} className="mb-2">✨</div>
          <h2 className="text-[18px] font-medium mb-1" style={{ color: 'var(--ink)' }}>All clean</h2>
          <p className="text-sm" style={{ color: 'var(--ink-3)' }}>Every ticket has what Meridian needs. Nothing to do.</p>
        </div>
      )}

      {fixTask && <HygieneDialog task={fixTask} onClose={() => { setFixTask(null); load() }} />}
    </div>
  )
}
