//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The selected task's detail pane, right column of TasksPanel — restyled
// with the timeline's mt-*/--t-*/--color-state-* tokens (was TasksView's
// inline TaskDetail, old --ink/--paper palette). Meridian-native time stats
// (today/week/sessions + the session list) live here since they aren't part
// of TaskDetailDialog's generic tracker metadata; opening the full
// description/acceptance-criteria view still goes through TaskDetailDialog
// via onOpenDetail so that dialog stays the one reusable "show me everything
// about this ticket" surface.

'use client'

import { fmtDur, fmtClock, AppGlyph, CatDot, CATS, PROVIDER_META } from '@/components/atoms'
import type { TaskSummary, TodayResponse } from '@/lib/api-types'

function isDueSoon(due: string): boolean {
  const ms = new Date(due + 'T00:00:00').getTime() - Date.now()
  return ms <= 3 * 86400000
}

export function TasksDetailPane({ task, sessions, epicColor, onFix, onOpenDetail }: {
  task: TaskSummary
  sessions: TodayResponse['sessions']
  epicColor: string
  onFix: () => void
  onOpenDetail: () => void
}) {
  const sortedSessions = [...sessions].sort((a, b) => b.started_at.localeCompare(a.started_at))
  const providerMeta = PROVIDER_META[task.provider]
  const hygieneCount = task.hygiene?.issues.length ?? 0

  return (
    <div className="space-y-6 min-w-0">
      <div>
        {task.epic_title && (
          <div className="flex items-center gap-2 mb-2.5">
            <span className="inline-block rounded-full shrink-0" style={{ width: 6, height: 6, background: epicColor }} />
            <span className="mt-label" style={{ color: epicColor }}>
              {task.epic_key && <span className="mt-mono-sm mr-1.5" style={{ opacity: 0.75 }}>{task.epic_key}</span>}
              {task.epic_title}
            </span>
          </div>
        )}

        <div className="flex items-center gap-2 flex-wrap mb-2.5">
          <span className="mt-mono-sm text-[11px] px-1.5 py-0.5 rounded bg-key-bg text-key-text">{task.key}</span>
          <span className="mt-body-sm inline-flex items-center gap-1.5" style={{ color: 'var(--t-muted)' }}>
            <span className="inline-block w-1.5 h-1.5 rounded-full"
              style={{ background: task.is_terminal ? 'var(--color-state-approved)' : 'var(--color-state-proposal)' }} />
            {task.status || '—'}
          </span>
          {task.issue_type && (
            <span className="mt-chip px-1.5 py-0.5 rounded" style={{ color: 'var(--t-muted)', border: '1px solid var(--t-hair)' }}>
              {task.issue_type}
            </span>
          )}
          {providerMeta && (
            <span className="mt-chip px-1.5 py-0.5 rounded" style={{ color: providerMeta.color, border: `1px solid ${providerMeta.color}` }}>
              {providerMeta.label}
            </span>
          )}
          <button onClick={onOpenDetail} className="mt-body-sm ml-auto shrink-0" style={{ color: 'var(--color-state-proposal)', fontWeight: 700 }}>
            Full details →
          </button>
        </div>

        <p className="mt-title-lg text-title">{task.title}</p>
        {task.description && (
          <p className="mt-body mt-2.5" style={{ color: 'var(--t-muted)' }}>{task.description}</p>
        )}
      </div>

      {hygieneCount > 0 && (
        <button onClick={onFix}
          className="w-full text-left rounded-xl px-4 py-3 flex items-center gap-3"
          style={{ background: 'color-mix(in srgb, var(--color-state-pending) 12%, transparent)', border: '1px solid color-mix(in srgb, var(--color-state-pending) 30%, transparent)' }}>
          <span style={{ color: 'var(--color-state-pending)' }}>⚠</span>
          <span className="min-w-0 flex-1">
            <p className="mt-card-title" style={{ color: 'var(--color-state-pending)' }}>
              {hygieneCount} fix{hygieneCount === 1 ? '' : 'es'} for a healthier ticket
            </p>
            <p className="mt-body-sm mt-0.5 truncate" style={{ color: 'var(--t-muted)' }}>
              {task.hygiene!.issues.map(i => i.hint).join(' · ')}
            </p>
          </span>
        </button>
      )}

      <div className="grid grid-cols-3 gap-3">
        <Stat label="Today" value={fmtDur(task.today_s)}
          hint={task.today_autonomous_s >= 60 ? `+${fmtDur(task.today_autonomous_s)} agent while away` : undefined} />
        <Stat label="This week" value={fmtDur(task.week_s)} />
        <Stat label="Sessions" value={String(task.session_count)} />
      </div>

      {(task.start_date || task.due_date) && (
        <div className="flex items-center gap-6">
          {task.start_date && <DateBit label="Start" value={task.start_date} />}
          {task.due_date && <DateBit label="Due" value={task.due_date} warn={isDueSoon(task.due_date)} />}
        </div>
      )}

      {sortedSessions.length > 0 ? (
        <div>
          <p className="mt-label mb-2.5" style={{ color: 'var(--t-faint)' }}>Sessions today</p>
          <div className="rounded-xl overflow-hidden bg-card" style={{ border: '1px solid var(--t-card-border)' }}>
            {sortedSessions.map((s, i) => (
              <div key={s.id} className="grid grid-cols-[auto_1fr_auto] items-center gap-3.5 px-4 py-3"
                style={{ borderTop: i > 0 ? '1px solid var(--t-hair)' : undefined }}>
                <AppGlyph app={s.app} size={22} />
                <div className="min-w-0">
                  <p className="mt-body-sm truncate" style={{ color: 'var(--t-title)' }}>{s.titles[0] || s.app}</p>
                  <div className="flex items-center gap-2 mt-0.5">
                    <span className="mt-mono-sm text-[11px]" style={{ color: 'var(--t-faint)' }}>{fmtClock(s.started_at)}</span>
                    <CatDot cat={s.cat} />
                    <span className="mt-body-sm" style={{ color: 'var(--t-faint)' }}>{CATS[s.cat]?.label ?? s.cat}</span>
                  </div>
                </div>
                <span className="mt-mono-sm text-[11px]" style={{ color: 'var(--t-muted)' }}>{fmtDur(s.dur)}</span>
              </div>
            ))}
          </div>
        </div>
      ) : task.today_s === 0 ? (
        <div className="py-10 text-center rounded-xl" style={{ border: '1px dashed var(--t-hair)' }}>
          <p className="mt-body-sm" style={{ color: 'var(--t-muted)' }}>No activity captured for this task today.</p>
        </div>
      ) : null}
    </div>
  )
}

function Stat({ label, value, hint }: { label: string; value: string; hint?: string }) {
  return (
    <div className="rounded-xl p-3.5 bg-box">
      <p className="mt-label mb-1.5" style={{ color: 'var(--t-faint)' }}>{label}</p>
      <p className="mt-stat" style={{ color: 'var(--t-title)' }}>{value}</p>
      {hint && <p className="mt-body-sm mt-1" style={{ color: 'var(--t-faint)' }}>{hint}</p>}
    </div>
  )
}

function DateBit({ label, value, warn }: { label: string; value: string; warn?: boolean }) {
  return (
    <div>
      <p className="mt-label mb-1" style={{ color: 'var(--t-faint)' }}>{label}</p>
      <p className="mt-mono-sm text-[13px]" style={{ color: warn ? 'var(--color-state-pending)' : 'var(--t-muted)' }}>{value}</p>
    </div>
  )
}
