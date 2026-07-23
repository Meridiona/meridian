//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Tasks — restyled with the timeline's mt-*/--t-*/--color-state-* tokens
// (was TasksView's --ink/--paper palette). Same master-list + sticky detail
// pane layout (kept — good for scanning many tickets side by side in a wide
// modal), decluttered: fewer borders/dividers, one clean epic-grouped list,
// a slim header (Refresh + Integrations only), provider tabs only when more
// than one tracker is connected. "Full details →" in the detail pane opens
// the shared TaskDetailDialog for the complete description/acceptance
// criteria; hygiene fixes still go through HygieneDialog (shared with the
// old fix flow — not touched here).

'use client'

import { useEffect, useState } from 'react'
import { ProviderGlyph, fmtDur } from '@/components/atoms'
import type { TaskSummary, TasksResponse, TodayResponse, IntegrationsResponse } from '@/lib/api-types'
import { load, mutate } from '@/lib/bridge'
import { filterByConnectedProviders } from '@/lib/integrations'
import HygieneDialog from '@/components/HygieneDialog'
import ConnectTrackers from '@/components/IntegrationConnect'
import { TasksDetailPane } from './TasksDetailPane'
import { useTaskStatusChange, StatusBanner } from './useTaskStatusChange'

const TASKS_POLL_INTERVAL_MS = 60_000

const EPIC_PALETTE = ['#8B5CF6', '#3B82F6', '#F97316', '#10B981', '#EF4444', '#EC4899', '#0EA5A0', '#EAB308']

function epicColor(epicKey: string | null): string {
  if (!epicKey) return 'var(--t-faint)'
  let h = 0
  for (let i = 0; i < epicKey.length; i++) h = (h * 31 + epicKey.charCodeAt(i)) >>> 0
  return EPIC_PALETTE[h % EPIC_PALETTE.length]
}

export function TasksPanel({ onOpenTask }: { onOpenTask: (key: string, title?: string) => void }) {
  const [data, setData] = useState<TasksResponse | null>(null)
  const [todaySessions, setTodaySessions] = useState<TodayResponse['sessions']>([])
  const [integrations, setIntegrations] = useState<IntegrationsResponse | null>(null)
  const [selected, setSelected] = useState<string | null>(null)
  const [syncing, setSyncing] = useState(false)
  const [syncError, setSyncError] = useState<string | null>(null)
  const [providerFilter, setProviderFilter] = useState<string>('all')
  const [showIntegrations, setShowIntegrations] = useState(false)
  const [fixTask, setFixTask] = useState<TaskSummary | null>(null)

  // Optimistically reflect a status change in the local list (a done/cancelled
  // status makes the task terminal, which drops it from the board).
  const patchTaskStatus = (key: string, status: string, isTerminal: boolean) =>
    setData(d => d ? { ...d, tasks: d.tasks.map(t => t.key === key ? { ...t, status, is_terminal: isTerminal } : t) } : d)

  const { statusBusyKey, undo, note, handleSetStatus, handleUndo, dismissUndo, dismissNote } =
    useTaskStatusChange(patchTaskStatus, { onUndoSettled: () => fetchTasks() })

  const fetchTasks = () => {
    load<TasksResponse>('/api/tasks', 'get_tasks').then((d) => {
      setData(d)
      setSelected(prev => prev ?? (d.tasks.find(t => t.today_s > 0) ?? d.tasks[0])?.key ?? null)
    }).catch(() => {})
  }

  const fetchIntegrations = () => {
    load<IntegrationsResponse>('/api/integrations', 'get_integrations').then(setIntegrations).catch(() => {})
  }

  const refreshBoard = () => { fetchTasks(); fetchIntegrations() }

  const handleSync = () => {
    if (syncing) return
    setSyncing(true); setSyncError(null)
    mutate('/api/tasks/sync', 'sync_tasks', {})
      .then(() => refreshBoard())
      .catch((e) => setSyncError(typeof e === 'string' ? e : e instanceof Error ? e.message : 'Sync failed'))
      .finally(() => setSyncing(false))
  }

  useEffect(() => {
    refreshBoard()
    load<TodayResponse>('/api/today', 'get_today').then(d => setTodaySessions(d.sessions ?? [])).catch(() => {})
    const timer = setInterval(refreshBoard, TASKS_POLL_INTERVAL_MS)
    return () => clearInterval(timer)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // TaskDetailDialog is a separate modal layered on top of this panel (it never
  // remounts this component), so a delete made there needs its own signal to
  // drop the row from this already-fetched list — see that dialog's `deleteTask`.
  useEffect(() => {
    const onDeleted = () => fetchTasks()
    window.addEventListener('meridian:task-deleted', onDeleted)
    return () => window.removeEventListener('meridian:task-deleted', onDeleted)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const header = (
    <div className="flex items-center justify-between mb-5">
      <p className="mt-body-sm" style={{ color: 'var(--t-faint)' }}>
        {data ? `${filterByConnectedProviders(data.tasks, integrations).filter(t => t.today_s > 0).length} touched today` : 'Loading…'}
      </p>
      <div className="flex items-center gap-2">
        <button onClick={handleSync} disabled={syncing}
          className="mt-body-sm px-3 py-1.5 rounded-md bg-ctrl inline-flex items-center gap-1.5"
          style={{ border: `1px solid ${syncError ? 'var(--color-state-pending)' : 'var(--t-ctrl-border)'}`, color: syncError ? 'var(--color-state-pending)' : 'var(--t-muted)', opacity: syncing ? 0.6 : 1 }}
          title="Pull latest tasks from your trackers">
          <span style={{ display: 'inline-block', animation: syncing ? 'spin 1s linear infinite' : 'none' }}>{syncError ? '⚠' : '↻'}</span>
          {syncing ? 'Syncing…' : syncError ? 'Sync failed' : 'Refresh'}
        </button>
        <button onClick={() => setShowIntegrations(s => !s)}
          className="mt-body-sm px-3 py-1.5 rounded-md bg-ctrl"
          style={{ border: '1px solid var(--t-ctrl-border)', color: showIntegrations ? 'var(--t-title)' : 'var(--t-muted)' }}>
          Integrations
        </button>
      </div>
    </div>
  )

  if (!data) return <div>{header}<p className="mt-body-sm italic" style={{ color: 'var(--t-faint-2)' }}>Loading…</p></div>

  if (showIntegrations) {
    return (
      <div>
        {header}
        <button onClick={() => setShowIntegrations(false)} className="mt-body-sm mb-4" style={{ color: 'var(--t-faint)' }}>← Back to tasks</button>
        <ConnectTrackers integrations={integrations} onChanged={fetchIntegrations} />
      </div>
    )
  }

  if (data.tasks.length === 0) {
    const anyConnected = integrations !== null && Object.entries(integrations).some(([k, v]) => k !== 'sync_errors' && Boolean(v))
    return (
      <div>
        {header}
        {anyConnected ? (
          <p className="mt-body-sm" style={{ color: 'var(--t-muted)' }}>
            No tasks assigned to you. Make sure issues in your tracker are assigned to your account, then hit Refresh.
          </p>
        ) : (
          <ConnectTrackers integrations={integrations} onChanged={fetchIntegrations} />
        )}
      </div>
    )
  }

  // Tasks belonging to a connected tracker — empty here means "no connected-provider
  // tasks", which is the ConnectTrackers prompt below.
  const connectedTasks = filterByConnectedProviders(data.tasks, integrations)
  if (connectedTasks.length === 0 && integrations !== null) {
    return <div>{header}<ConnectTrackers integrations={integrations} onChanged={fetchIntegrations} /></div>
  }

  // What's on the board — a completed or cleaned-up ticket isn't something you
  // act on here. It still counts toward "touched today"; it's just not listed.
  //
  // `on_board` is computed in Rust by the same predicate that builds the worklog
  // matcher's candidate set, so this list IS that list. Do not substitute
  // `!t.is_terminal`: that was the old filter, and it disagreed with the matcher
  // (which also drops cleanup-excluded tickets), so a task could sit here and be
  // invisible to the model with nothing saying so.
  const activeTasks = connectedTasks.filter(t => t.on_board)
  if (activeTasks.length === 0) {
    return (
      <div>
        {header}
        <p className="mt-body-sm" style={{ color: 'var(--t-muted)' }}>
          All your tasks are done - nothing active right now. Completed tickets are hidden here.
        </p>
        <StatusBanner undo={undo} note={note} busy={!!statusBusyKey}
          onUndo={handleUndo} onDismissUndo={dismissUndo} onDismissNote={dismissNote} />
      </div>
    )
  }

  const presentProviders = Array.from(new Set(activeTasks.map(t => t.provider))).sort()
  const showTabs = presentProviders.length > 1
  const effectiveFilter = providerFilter === 'all' || presentProviders.includes(providerFilter) ? providerFilter : 'all'
  const visibleTasks = effectiveFilter === 'all' ? activeTasks : activeTasks.filter(t => t.provider === effectiveFilter)
  const sel = visibleTasks.find(t => t.key === selected) ?? visibleTasks[0]

  const epicOrder: Array<{ key: string; title: string | null }> = []
  const byEpic: Record<string, TaskSummary[]> = {}
  for (const t of visibleTasks) {
    const eKey = t.epic_key ?? '__none__'
    if (!byEpic[eKey]) { epicOrder.push({ key: eKey, title: t.epic_title ?? null }); byEpic[eKey] = [] }
    byEpic[eKey].push(t)
  }

  return (
    <div>
      {header}

      {showTabs && (
        <div className="flex items-center gap-1.5 mb-4">
          <Tab id="all" active={effectiveFilter === 'all'} onClick={() => setProviderFilter('all')} />
          {presentProviders.map(p => <Tab key={p} id={p} active={effectiveFilter === p} onClick={() => setProviderFilter(p)} />)}
        </div>
      )}

      <div className="grid grid-cols-1 lg:grid-cols-[minmax(0,300px)_minmax(0,1fr)] gap-6 items-start">
        <div className="rounded-xl overflow-hidden bg-card" style={{ border: '1px solid var(--t-card-border)' }}>
          {epicOrder.map(({ key: eKey, title: epicTitle }) => {
            const group = byEpic[eKey] ?? []
            const color = epicColor(eKey === '__none__' ? null : eKey)
            return (
              <div key={eKey}>
                <div className="px-4 py-2 flex items-center gap-2" style={{ borderLeft: `3px solid ${color}`, background: 'var(--t-box)' }}>
                  <span className="mt-label truncate" style={{ color }}>{epicTitle ?? 'No epic'}</span>
                  <span className="mt-mono-sm text-[10px] ml-auto shrink-0" style={{ color: 'var(--t-faint)' }}>{group.length}</span>
                </div>
                {group.map(t => (
                  <TaskRow key={t.key} task={t} selected={t.key === selected} onSelect={() => setSelected(t.key)}
                    onFix={() => setFixTask(t)} accent={color} showProvider={showTabs} />
                ))}
              </div>
            )
          })}
        </div>

        {sel && (
          <div className="lg:sticky lg:top-0">
            <TasksDetailPane
              task={sel}
              sessions={todaySessions.filter(s => s.task_key === sel.key)}
              epicColor={epicColor(sel.epic_key)}
              onFix={() => setFixTask(sel)}
              onOpenDetail={() => onOpenTask(sel.key, sel.title)}
              onSetStatus={handleSetStatus}
              statusBusy={statusBusyKey === sel.key}
            />
          </div>
        )}
      </div>

      {fixTask && <HygieneDialog task={fixTask} onClose={() => setFixTask(null)} onApplied={fetchTasks} />}
      <StatusBanner undo={undo} note={note} busy={!!statusBusyKey}
        onUndo={handleUndo} onDismissUndo={dismissUndo} onDismissNote={dismissNote} />
    </div>
  )
}

function Tab({ id, active, onClick }: { id: string; active: boolean; onClick: () => void }) {
  const label = id === 'all' ? 'All' : id
  return (
    <button onClick={onClick}
      className="mt-body-sm px-3 py-1.5 rounded-md inline-flex items-center gap-1.5"
      style={{
        background: active ? 'var(--t-box)' : 'transparent',
        border: `1px solid ${active ? 'var(--t-hair)' : 'transparent'}`,
        color: active ? 'var(--t-title)' : 'var(--t-faint)',
      }}>
      {id !== 'all' && <ProviderGlyph provider={id} size={14} />}
      {label}
    </button>
  )
}

function TaskRow({ task, selected, onSelect, onFix, accent, showProvider }: {
  task: TaskSummary; selected: boolean; onSelect: () => void; onFix: () => void; accent: string; showProvider: boolean
}) {
  return (
    <div role="button" tabIndex={0} onClick={onSelect}
      onKeyDown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); onSelect() } }}
      className="w-full text-left px-4 py-3 border-t cursor-pointer"
      style={{ background: selected ? 'var(--t-row-hover)' : 'transparent', borderColor: 'var(--t-hair)', borderLeft: `2px solid ${selected ? accent : 'transparent'}` }}>
      <div className="flex items-center gap-2.5">
        {showProvider && <ProviderGlyph provider={task.provider} size={16} />}
        <span className="mt-mono-sm text-[11px] px-1.5 py-0.5 rounded bg-key-bg text-key-text shrink-0">{task.key}</span>
        {task.hygiene && task.hygiene.issues.length > 0 && (
          <button type="button"
            onClick={(e) => { e.stopPropagation(); onFix() }}
            className="mt-mono-sm text-[10px] px-1.5 py-0.5 rounded-full shrink-0 cursor-pointer border-0"
            style={{ background: 'color-mix(in srgb, var(--color-state-pending) 16%, transparent)', color: 'var(--color-state-pending)' }}
            title={`${task.hygiene.issues.length} hygiene fixes — click to fix`}>
            ⚠ {task.hygiene.issues.length}
          </button>
        )}
        <span className="mt-mono-sm text-[11px] ml-auto shrink-0" style={{ color: task.today_s > 0 ? 'var(--t-muted)' : 'var(--t-faint-2)' }}>
          {task.today_s > 0 ? fmtDur(task.today_s) : '—'}
        </span>
      </div>
      <p className="mt-body-sm mt-1.5 truncate" style={{ color: 'var(--t-title)' }}>{task.title}</p>
    </div>
  )
}
