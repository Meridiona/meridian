//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

import { useEffect, useRef, useState } from 'react'
import { fmtDur, fmtClock, AppGlyph, CatDot, TaskKey, StatusPill, SectionHead, Card, CATS, PROVIDER_META } from '@/components/atoms'
import type { TaskSummary, TasksResponse } from '@/lib/api-types'
import { load, mutate } from '@/lib/bridge'
import { filterByConnectedProviders } from '@/lib/integrations'
import HygieneDialog from '@/components/HygieneDialog'
import ConnectTrackers from '@/components/IntegrationConnect'
import type { TodayResponse } from '@/lib/api-types'
import type { IntegrationsResponse } from '@/lib/api-types'

const TASKS_POLL_INTERVAL_MS = 60_000


// Deterministic color from epic title — cycles through a palette of muted hues
const EPIC_PALETTE = [
  '#7C6FCD', // purple
  '#2684FF', // blue
  '#E66F2E', // orange
  '#2D9B6A', // green
  '#C0392B', // red
  '#8E6BBF', // violet
  '#1A7A8A', // teal
  '#B7860C', // gold
]

// covers overdue AND due within 3 days
function isDueSoon(due: string): boolean {
  const ms = new Date(due + 'T00:00:00').getTime() - Date.now()
  return ms <= 3 * 86400000
}

function epicColor(epicKey: string | null): string {
  if (!epicKey) return 'var(--ink-4)'
  let h = 0
  for (let i = 0; i < epicKey.length; i++) h = (h * 31 + epicKey.charCodeAt(i)) >>> 0
  return EPIC_PALETTE[h % EPIC_PALETTE.length]
}

export default function TasksView({ focusKey, openIntegrations }: { focusKey?: string | null; openIntegrations?: boolean }) {
  const [data, setData] = useState<TasksResponse | null>(null)
  const [todaySessions, setTodaySessions] = useState<TodayResponse['sessions']>([])
  const [integrations, setIntegrations] = useState<IntegrationsResponse | null>(null)
  const [selected, setSelected] = useState<string | null>(focusKey ?? null)
  const [syncing, setSyncing] = useState(false)
  const [lastSynced, setLastSynced] = useState<Date | null>(null)
  const [syncError, setSyncError] = useState<string | null>(null)
  const [providerFilter, setProviderFilter] = useState<string>('all')
  const [showIntegrations, setShowIntegrations] = useState(openIntegrations ?? false)
  const [fixTask, setFixTask] = useState<TaskSummary | null>(null)
  const [collapsedEpics, setCollapsedEpics] = useState<Set<string>>(new Set())

  const fetchTasks = () => {
    // get_tasks (Rust) in the Tauri window, /api/tasks in a browser — same shape.
    load<TasksResponse>('/api/tasks', 'get_tasks').then((d) => {
      setData(d)
      if (!selected && d.tasks.length > 0) {
        const first = d.tasks.find(t => t.today_s > 0) ?? d.tasks[0]
        setSelected(first.key)
      }
    }).catch(() => {})
  }

  const fetchIntegrations = () => {
    // get_integrations (Rust) in the Tauri window, /api/integrations in a browser.
    load<IntegrationsResponse>('/api/integrations', 'get_integrations').then((d) => {
      setIntegrations(d)
    }).catch(() => {})
  }

  const handleSync = () => {
    if (syncing) return
    setSyncing(true)
    setSyncError(null)
    // Dual-path: sync_tasks (Rust) in the app, /api/tasks/sync POST in a browser.
    // mutate throws the CLI's stderr on failure; success re-fetches the board.
    mutate('/api/tasks/sync', 'sync_tasks', {})
      .then(() => { setLastSynced(new Date()); fetchTasks() })
      .catch((e) => setSyncError(typeof e === 'string' ? e : e instanceof Error ? e.message : 'Sync failed — check daemon logs'))
      .finally(() => setSyncing(false))
  }

  useEffect(() => {
    fetchTasks()
    load<TodayResponse>('/api/today', 'get_today').then((d) => {
      setTodaySessions(d.sessions ?? [])
    }).catch(() => {})
    fetchIntegrations()

    const timer = setInterval(fetchTasks, TASKS_POLL_INTERVAL_MS)
    return () => { clearInterval(timer) }
  }, []) // eslint-disable-line react-hooks/exhaustive-deps

  if (!data) {
    return (
      <div className="space-y-8">
        <TasksHeader syncing={syncing} syncError={syncError} lastSynced={lastSynced} onSync={handleSync} onIntegrations={() => setShowIntegrations(s => !s)} showIntegrations={showIntegrations} />
        <p className="text-[13px]" style={{ color: 'var(--ink-3)' }}>Loading…</p>
      </div>
    )
  }

  if (data.tasks.length === 0) {
    // If at least one provider is connected but the board is empty, the user
    // has synced but nothing is assigned to them — show a targeted hint rather
    // than the connect panel which implies nothing is wired up at all.
    const anyConnected = integrations !== null && Object.values(integrations).some(Boolean)
    return (
      <div className="space-y-8">
        <TasksHeader syncing={syncing} syncError={syncError} lastSynced={lastSynced} onSync={handleSync} onIntegrations={() => setShowIntegrations(s => !s)} showIntegrations={showIntegrations} />
        {anyConnected ? (
          <div style={{ paddingTop: 8 }}>
            <p className="text-[13px]" style={{ color: 'var(--ink-2)', marginBottom: 6 }}>No tasks assigned to you.</p>
            <p className="text-[12px]" style={{ color: 'var(--ink-3)' }}>
              Make sure issues in your tracker are assigned to your account, then hit Refresh.
            </p>
          </div>
        ) : (
          <ConnectTrackers integrations={integrations} onChanged={fetchIntegrations} />
        )}
      </div>
    )
  }

  // Only include tasks from providers that are currently connected. While
  // integrations is still loading (null), show everything to avoid a flash.
  const activeTasks = filterByConnectedProviders(data.tasks, integrations)

  // All providers disconnected: show the connect panel instead of a blank detail pane.
  if (activeTasks.length === 0 && integrations !== null) {
    return (
      <div className="space-y-8">
        <TasksHeader syncing={syncing} syncError={syncError} lastSynced={lastSynced} onSync={handleSync} onIntegrations={() => setShowIntegrations(s => !s)} showIntegrations={showIntegrations} />
        <ConnectTrackers integrations={integrations} onChanged={fetchIntegrations} />
      </div>
    )
  }

  const touched = activeTasks.filter(t => t.today_s > 0).length

  // Derive the set of providers actually present in the active task list.
  const presentProviders = Array.from(new Set(activeTasks.map(t => t.provider))).sort()
  const showProviderTabs = presentProviders.length > 1
  // If the selected provider was disconnected, fall back to 'all' so the list
  // never goes blank while the tabs are hidden.
  const effectiveProviderFilter =
    providerFilter === 'all' || presentProviders.includes(providerFilter)
      ? providerFilter
      : 'all'

  const visibleTasks = effectiveProviderFilter === 'all'
    ? activeTasks
    : activeTasks.filter(t => t.provider === effectiveProviderFilter)

  // activeTasks is non-empty here (empty case returned above), so visibleTasks[0]
  // is always defined unless the active provider filter matches nothing.
  const sel = visibleTasks.find(t => t.key === selected) ?? visibleTasks[0] ?? activeTasks[0]

  // Group tasks by epic_key (stable per-epic, not per-title) so cross-provider
  // epics with the same title don't collide.
  const epicOrder: Array<{ key: string; title: string | null }> = []
  const tasksByEpic: Record<string, TaskSummary[]> = {}
  for (const t of visibleTasks) {
    const eKey = t.epic_key ?? '__none__'
    if (!tasksByEpic[eKey]) {
      epicOrder.push({ key: eKey, title: t.epic_title ?? null })
      tasksByEpic[eKey] = []
    }
    tasksByEpic[eKey].push(t)
  }

  const toggleEpic = (eKey: string) => setCollapsedEpics(prev => {
    const next = new Set(prev)
    if (next.has(eKey)) next.delete(eKey); else next.add(eKey)
    return next
  })

  return (
    <div className="space-y-8">
      <TasksHeader
        syncing={syncing} syncError={syncError} lastSynced={lastSynced}
        onSync={handleSync} onIntegrations={() => setShowIntegrations(s => !s)}
        showIntegrations={showIntegrations} touched={touched}
      />

      {showIntegrations ? (
        <div>
          <button
            onClick={() => setShowIntegrations(false)}
            className="flex items-center gap-1.5 text-[12px] mb-5"
            style={{ color: 'var(--ink-3)', cursor: 'pointer', background: 'none', border: 'none', padding: 0 }}
          >
            ← Back to tasks
          </button>
          <ConnectTrackers integrations={integrations} onChanged={fetchIntegrations} />
        </div>
      ) : (
        <>
          {showProviderTabs && (
            <div className="flex items-center gap-1">
              <ProviderTab id="all" active={effectiveProviderFilter === 'all'} onClick={() => setProviderFilter('all')} />
              {presentProviders.map(p => (
                <ProviderTab key={p} id={p} active={effectiveProviderFilter === p} onClick={() => setProviderFilter(p)} />
              ))}
            </div>
          )}

          <div className="grid grid-cols-1 lg:grid-cols-[minmax(0,300px)_minmax(0,1fr)] gap-8 items-start">
            <div className="rounded-xl overflow-hidden border" style={{ borderColor: 'var(--rule)' }}>
              {epicOrder.map(({ key: eKey, title: epicTitle }, ei) => {
                const group = tasksByEpic[eKey] ?? []
                const color = epicColor(eKey === '__none__' ? null : eKey)
                const collapsed = collapsedEpics.has(eKey)
                return (
                  <div key={eKey}>
                    <button
                      onClick={() => toggleEpic(eKey)}
                      className="w-full flex items-center gap-2 px-4 py-2 text-left"
                      style={{
                        background: epicTitle ? color + '22' : 'var(--surface-2)',
                        borderTop: ei > 0 ? `1px solid ${color}33` : undefined,
                        borderLeft: `3px solid ${color}`,
                        cursor: 'pointer',
                      }}
                    >
                      <span
                        className="shrink-0 text-[9px] transition-transform"
                        style={{ color, transform: collapsed ? 'rotate(-90deg)' : 'rotate(0deg)', display: 'inline-block' }}
                      >▾</span>
                      <span className="text-[10px] font-semibold uppercase tracking-[0.15em] truncate" style={{ color }}>
                        {epicTitle ?? 'No epic'}
                      </span>
                      <span className="ml-auto font-mono tnum text-[10px] shrink-0" style={{ color: color + 'AA' }}>{group.length}</span>
                    </button>
                    {!collapsed && group.map(t => (
                      <TaskRow key={t.key} task={t} selected={t.key === selected} onSelect={() => setSelected(t.key)} onFix={() => setFixTask(t)} epicColor={color} showProvider={showProviderTabs} />
                    ))}
                  </div>
                )
              })}
            </div>

            {sel && (
              <div className="lg:sticky lg:top-8">
                <TaskDetail task={sel} sessions={todaySessions.filter(s => s.task_key === sel.key)} onFix={() => setFixTask(sel)} />
              </div>
            )}
          </div>
        </>
      )}

      {fixTask && <HygieneDialog task={fixTask} onClose={() => setFixTask(null)} onApplied={fetchTasks} />}
    </div>
  )
}

function TasksHeader({
  syncing, syncError, lastSynced, onSync, onIntegrations, showIntegrations, touched,
}: {
  syncing: boolean
  syncError: string | null
  lastSynced: Date | null
  onSync: () => void
  onIntegrations: () => void
  showIntegrations: boolean
  touched?: number
}) {
  return (
    <header className="rise flex items-end justify-between">
      <div>
        <p className="text-[11px] uppercase tracking-[0.2em]" style={{ color: 'var(--ink-3)' }}>Tasks</p>
        <h1 className="type-title mt-1" style={{ color: 'var(--ink)' }}>What you&apos;re working on</h1>
      </div>
      <div className="flex items-center gap-4">
        {touched !== undefined && (
          <p className="text-[12px]" style={{ color: 'var(--ink-3)' }}>
            <span className="font-mono tnum">{touched}</span> touched today
          </p>
        )}
        <div className="flex flex-col items-end gap-1">
          <button
            onClick={onSync}
            disabled={syncing}
            className="flex items-center gap-1.5 text-[12px] px-3 py-1.5 rounded-md transition-colors"
            style={{
              color: syncing ? 'var(--ink-4)' : syncError ? '#e53e3e' : 'var(--ink-3)',
              background: 'var(--surface)',
              border: `1px solid ${syncError ? '#e53e3e' : 'var(--rule)'}`,
              cursor: syncing ? 'not-allowed' : 'pointer',
            }}
            title={lastSynced ? `Last synced ${lastSynced.toLocaleTimeString()}` : 'Pull latest tasks from Jira / Linear / GitHub'}
          >
            <span style={{ display: 'inline-block', animation: syncing ? 'spin 1s linear infinite' : 'none' }}>
              {syncError ? '⚠' : '↻'}
            </span>
            {syncing ? 'Syncing…' : syncError ? 'Sync failed' : 'Refresh'}
          </button>
          {syncError && (
            <p className="text-[11px] max-w-[280px] text-right" style={{ color: '#e53e3e' }}>
              {syncError}
            </p>
          )}
        </div>
        <button
          onClick={onIntegrations}
          className="flex items-center gap-1.5 text-[12px] px-3 py-1.5 rounded-md transition-colors"
          style={{
            color: showIntegrations ? 'var(--ink)' : 'var(--ink-3)',
            background: showIntegrations ? 'var(--surface-2)' : 'var(--surface)',
            border: '1px solid var(--rule)',
            cursor: 'pointer',
          }}
        >
          Integrations
        </button>
      </div>
    </header>
  )
}

function ProviderTab({ id, active, onClick }: { id: string; active: boolean; onClick: () => void }) {
  const meta = PROVIDER_META[id]
  const label = id === 'all' ? 'All' : (meta?.label ?? id)
  const color = meta?.color
  return (
    <button
      onClick={onClick}
      className="flex items-center gap-1.5 text-[12px] px-3 py-1.5 rounded-md transition-colors"
      style={{
        background: active ? 'var(--surface-2)' : 'transparent',
        border: active ? '1px solid var(--rule)' : '1px solid transparent',
        color: active ? (color ?? 'var(--ink)') : 'var(--ink-3)',
        fontWeight: active ? 500 : 400,
      }}
    >
      {id !== 'all' && meta && (
        <span
          className="inline-flex items-center justify-center rounded shrink-0 font-mono"
          style={{ width: 14, height: 14, fontSize: 8, fontWeight: 700, background: meta.color + '1A', color: meta.color }}
        >
          {meta.glyph}
        </span>
      )}
      {label}
    </button>
  )
}

function TaskRow({ task, selected, onSelect, onFix, epicColor: eColor, showProvider }: { task: TaskSummary; selected: boolean; onSelect: () => void; onFix: () => void; epicColor?: string; showProvider?: boolean }) {
  const meta = PROVIDER_META[task.provider]
  const borderColor = selected ? (eColor ?? 'var(--accent)') : 'transparent'
  return (
    <button onClick={onSelect}
      className="w-full text-left px-4 py-3 transition-colors"
      style={{
        background: selected ? 'var(--surface-2)' : 'var(--surface)',
        borderLeft: `2px solid ${borderColor}`,
      }}>
      <div className="flex items-center gap-3">
        {showProvider && meta && (
          <span
            className="inline-flex items-center justify-center rounded shrink-0 font-mono"
            style={{ width: 16, height: 16, fontSize: 9, fontWeight: 700, background: meta.color + '1A', color: meta.color }}
            title={meta.label}
          >
            {meta.glyph}
          </span>
        )}
        <TaskKey keyId={task.key} />
        <StatusPill status={task.status} isTerminal={task.is_terminal} />
        {task.hygiene && task.hygiene.issues.length > 0 && (
          <span role="button" tabIndex={0}
            onClick={(e) => { e.stopPropagation(); onFix() }}
            onKeyDown={(e) => { if (e.key === 'Enter') { e.stopPropagation(); onFix() } }}
            className="inline-flex items-center gap-1 text-[10px] px-1.5 py-0.5 rounded-full shrink-0 cursor-pointer"
            style={{ background: 'var(--warn)' + '1A', color: 'var(--warn)' }}
            title={`${task.hygiene.issues.length} hygiene fix${task.hygiene.issues.length === 1 ? '' : 'es'} — click to fix`}>
            ⚠ {task.hygiene.issues.length}
          </span>
        )}
        <span className="ml-auto font-mono tnum text-[12px]" style={{ color: task.today_s > 0 ? 'var(--ink)' : 'var(--ink-4)' }}>
          {task.today_s > 0 ? fmtDur(task.today_s) : '—'}
        </span>
      </div>
      <p className="text-[13px] mt-1.5 truncate" style={{ color: 'var(--ink)' }}>{task.title}</p>
    </button>
  )
}

// A compact board-hygiene call-to-action in the detail pane: opens the focused
// fix dialog. The list of defects + controls lives in the dialog itself.
function HygienePanel({ task, onFix }: { task: TaskSummary; onFix: () => void }) {
  const h = task.hygiene
  if (!h || h.issues.length === 0) return null
  return (
    <button onClick={onFix}
      className="w-full rounded-xl border px-4 py-3 flex items-center gap-3 text-left transition-colors"
      style={{ borderColor: 'var(--warn)', background: 'var(--warn)' + '0F' }}>
      <span style={{ color: 'var(--warn)' }}>⚠</span>
      <div className="min-w-0 flex-1">
        <p className="text-[13px] font-medium" style={{ color: 'var(--ink)' }}>
          {h.issues.length} fix{h.issues.length === 1 ? '' : 'es'} for a healthier ticket
        </p>
        <p className="text-[11px] truncate" style={{ color: 'var(--ink-3)' }}>
          {h.issues.map(i => i.hint).join(' · ')}
        </p>
      </div>
      <span className="text-[12px] px-2.5 py-1 rounded-md shrink-0" style={{ background: 'var(--warn)', color: '#fff' }}>
        Review & fix
      </span>
    </button>
  )
}

function TaskDetail({ task, sessions, onFix }: { task: TaskSummary; sessions: TodayResponse['sessions']; onFix: () => void }) {
  const sortedSessions = [...sessions].sort((a, b) => b.started_at.localeCompare(a.started_at))
  const providerMeta = PROVIDER_META[task.provider]
  const eColor = epicColor(task.epic_title)

  return (
    <div className="space-y-7 min-w-0">
      <div>
        {task.epic_title && (
          <div className="flex items-center gap-2 mb-3">
            <span className="inline-block rounded-full shrink-0" style={{ width: 7, height: 7, background: eColor }} />
            <span className="text-[11px] uppercase tracking-[0.14em]" style={{ color: eColor }}>
              {task.epic_key && <span className="font-mono mr-1.5" style={{ opacity: 0.7 }}>{task.epic_key}</span>}
              {task.epic_title}
            </span>
          </div>
        )}
        <div className="flex items-center gap-3 mb-3">
          <TaskKey keyId={task.key} big />
          <StatusPill status={task.status} isTerminal={task.is_terminal} />
          {task.issue_type && (
            <span className="text-[11px] px-1.5 py-0.5 rounded-md" style={{ background: 'var(--tint)', color: 'var(--ink-2)' }}>
              {task.issue_type}
            </span>
          )}
          {providerMeta ? (
            <span
              className="inline-flex items-center gap-1.5 text-[11px] px-2 py-0.5 rounded-full"
              style={{ background: providerMeta.color + '1A', color: providerMeta.color, fontWeight: 500 }}
            >
              <span className="font-mono" style={{ fontSize: 9, fontWeight: 700 }}>{providerMeta.glyph}</span>
              {providerMeta.label}
            </span>
          ) : (
            <span className="text-[11px]" style={{ color: 'var(--ink-3)' }}>{task.provider}</span>
          )}
          {task.url && (
            <a href={task.url} target="_blank" rel="noopener noreferrer"
              className="ml-auto text-[12px]" style={{ color: 'var(--ink-3)' }}>
              Open ↗
            </a>
          )}
        </div>
        <h2 className="type-heading" style={{ color: 'var(--ink)' }}>
          {task.title}
        </h2>
        {task.description && (
          <p className="text-[14px] mt-3 max-w-prose" style={{ color: 'var(--ink-2)' }}>{task.description}</p>
        )}
      </div>

      <HygienePanel task={task} onFix={onFix} />

      <div className="grid grid-cols-3 rule-t rule-b" style={{ borderColor: 'var(--rule)' }}>
        <div className="px-5 py-4">
          <p className="text-[10px] uppercase tracking-[0.16em] mb-2" style={{ color: 'var(--ink-3)' }}>Today</p>
          <p className="font-mono tnum text-[22px] leading-none" style={{ color: 'var(--ink)' }}>{fmtDur(task.today_s)}</p>
          {task.today_autonomous_s >= 60 && (
            <p className="text-[10px] mt-1.5" style={{ color: 'var(--live)' }}
              title="Of today's total, the agent ran on its own while you were away — the part that adds time beyond your own.">
              +{fmtDur(task.today_autonomous_s)} agent while away
            </p>
          )}
        </div>
        <div className="px-5 py-4 rule-l" style={{ borderLeftColor: 'var(--rule)' }}>
          <p className="text-[10px] uppercase tracking-[0.16em] mb-2" style={{ color: 'var(--ink-3)' }}>This week</p>
          <p className="font-mono tnum text-[22px] leading-none" style={{ color: 'var(--ink)' }}>{fmtDur(task.week_s)}</p>
        </div>
        <div className="px-5 py-4 rule-l" style={{ borderLeftColor: 'var(--rule)' }}>
          <p className="text-[10px] uppercase tracking-[0.16em] mb-2" style={{ color: 'var(--ink-3)' }}>Sessions</p>
          <p className="font-mono tnum text-[22px] leading-none" style={{ color: 'var(--ink)' }}>{task.session_count}</p>
        </div>
      </div>

      {(task.start_date || task.due_date) && (
        <div className="flex items-center gap-6">
          {task.start_date && (
            <div>
              <p className="text-[10px] uppercase tracking-[0.16em] mb-1" style={{ color: 'var(--ink-3)' }}>Start</p>
              <p className="font-mono tnum text-[13px]" style={{ color: 'var(--ink-2)' }}>{task.start_date}</p>
            </div>
          )}
          {task.due_date && (
            <div>
              <p className="text-[10px] uppercase tracking-[0.16em] mb-1" style={{ color: 'var(--ink-3)' }}>Due</p>
              <p className="font-mono tnum text-[13px]" style={{ color: isDueSoon(task.due_date) ? '#e53e3e' : 'var(--ink-2)' }}>
                {task.due_date}
              </p>
            </div>
          )}
        </div>
      )}

      {sortedSessions.length > 0 ? (
        <div>
          <p className="text-[10px] uppercase tracking-[0.16em] mb-3" style={{ color: 'var(--ink-3)' }}>Sessions today</p>
          <div className="rule rounded-xl border overflow-hidden" style={{ borderColor: 'var(--rule)' }}>
            {sortedSessions.map((s, i) => (
              <div key={s.id}
                className={`grid grid-cols-[auto_1fr_auto] items-center gap-4 px-4 py-3 ${i > 0 ? 'rule-t' : ''}`}
                style={{ borderTopColor: 'var(--rule)', background: 'var(--surface)' }}>
                <AppGlyph app={s.app} size={22} />
                <div className="min-w-0">
                  <p className="text-[13px] truncate" style={{ color: 'var(--ink)' }}>{s.titles[0] || s.app}</p>
                  <div className="flex items-center gap-2 mt-0.5">
                    <span className="font-mono tnum text-[11px]" style={{ color: 'var(--ink-3)' }}>{fmtClock(s.started_at)}</span>
                    <CatDot cat={s.cat} />
                    <span className="text-[11px]" style={{ color: 'var(--ink-3)' }}>{CATS[s.cat]?.label ?? s.cat}</span>
                  </div>
                </div>
                <span className="font-mono tnum text-[12px]" style={{ color: 'var(--ink-2)' }}>{fmtDur(s.dur)}</span>
              </div>
            ))}
          </div>
        </div>
      ) : task.today_s === 0 ? (
        <div className="py-12 text-center rule rounded-xl border" style={{ borderColor: 'var(--rule)', background: 'var(--surface)' }}>
          <p className="text-[13px]" style={{ color: 'var(--ink-3)' }}>No activity captured for this task today.</p>
        </div>
      ) : null}

      {task.today_s > 0 && (
        <Card className="p-5">
          <SectionHead kicker="Suggested worklog" title={`Log ${fmtDur(task.today_s)} to ${task.key}`} />
          <div className="flex items-center gap-3 mt-3">
            <button className="text-[12px] px-3 py-1.5 rounded-md font-medium"
              style={{ color: 'var(--paper)', background: 'var(--ink)' }}>
              Log to {task.provider === 'jira' ? 'Jira' : task.provider}
            </button>
            <button className="text-[12px] px-3 py-1.5 rounded-md" style={{ color: 'var(--ink-3)' }}>
              Edit draft
            </button>
          </div>
        </Card>
      )}
    </div>
  )
}


// keep this export for compat
export { AppGlyph }
