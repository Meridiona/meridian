//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

import { useEffect, useState } from 'react'
import { fmtDur, fmtClock, AppGlyph, CatDot, TaskKey, StatusPill, SectionHead, Card, CATS, PROVIDER_META } from '@/components/atoms'
import type { TaskSummary, TasksResponse } from '@/app/api/tasks/route'
import type { TodayResponse } from '@/app/api/today/route'
import type { IntegrationsResponse } from '@/app/api/integrations/route'

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

export default function TasksView({ focusKey }: { focusKey?: string | null }) {
  const [data, setData] = useState<TasksResponse | null>(null)
  const [todaySessions, setTodaySessions] = useState<TodayResponse['sessions']>([])
  const [integrations, setIntegrations] = useState<IntegrationsResponse | null>(null)
  const [selected, setSelected] = useState<string | null>(focusKey ?? null)
  const [syncing, setSyncing] = useState(false)
  const [lastSynced, setLastSynced] = useState<Date | null>(null)
  const [syncError, setSyncError] = useState<string | null>(null)
  const [providerFilter, setProviderFilter] = useState<string>('all')
  const [showIntegrations, setShowIntegrations] = useState(false)
  const [collapsedEpics, setCollapsedEpics] = useState<Set<string>>(new Set())

  const fetchTasks = () => {
    fetch('/api/tasks').then(r => r.json()).then((d: TasksResponse) => {
      setData(d)
      if (!selected && d.tasks.length > 0) {
        const first = d.tasks.find(t => t.today_s > 0) ?? d.tasks[0]
        setSelected(first.key)
      }
    }).catch(() => {})
  }

  const fetchIntegrations = () => {
    fetch('/api/integrations').then(r => r.json()).then((d: IntegrationsResponse) => {
      setIntegrations(d)
    }).catch(() => {})
  }

  const handleSync = () => {
    if (syncing) return
    setSyncing(true)
    setSyncError(null)
    fetch('/api/tasks/sync', { method: 'POST' })
      .then(async r => {
        const body = await r.json().catch(() => ({}))
        if (!r.ok || body.ok === false) {
          setSyncError(body.error ?? 'Sync failed — check daemon logs')
        } else {
          setLastSynced(new Date())
          fetchTasks()
        }
      })
      .catch(() => setSyncError('Could not reach the daemon — is it running?'))
      .finally(() => setSyncing(false))
  }

  useEffect(() => {
    fetchTasks()
    fetch('/api/today').then(r => r.json()).then((d: TodayResponse) => {
      setTodaySessions(d.sessions ?? [])
    }).catch(() => {})
    fetchIntegrations()

    const timer = setInterval(fetchTasks, TASKS_POLL_INTERVAL_MS)
    return () => { clearInterval(timer) }
  }, []) // eslint-disable-line react-hooks/exhaustive-deps

  if (!data) {
    return (
      <div className="space-y-8">
        <header className="rise">
          <p className="text-[11px] uppercase tracking-[0.2em]" style={{ color: 'var(--ink-3)' }}>Tasks</p>
          <h1 className="type-title mt-1" style={{ color: 'var(--ink)' }}>What you&apos;re working on</h1>
        </header>
        <p className="text-[13px]" style={{ color: 'var(--ink-3)' }}>Loading…</p>
      </div>
    )
  }

  if (data.tasks.length === 0) {
    return (
      <div className="space-y-8">
        <header className="rise">
          <p className="text-[11px] uppercase tracking-[0.2em]" style={{ color: 'var(--ink-3)' }}>Tasks</p>
          <h1 className="type-title mt-1" style={{ color: 'var(--ink)' }}>What you&apos;re working on</h1>
        </header>
        <ConnectTrackers integrations={integrations} onDisconnect={fetchIntegrations} />
      </div>
    )
  }

  const touched = data.tasks.filter(t => t.today_s > 0).length

  // Derive the set of providers actually present in the task list.
  const presentProviders = Array.from(new Set(data.tasks.map(t => t.provider))).sort()
  const showProviderTabs = presentProviders.length > 1

  const visibleTasks = providerFilter === 'all'
    ? data.tasks
    : data.tasks.filter(t => t.provider === providerFilter)

  const sel = visibleTasks.find(t => t.key === selected) ?? visibleTasks[0] ?? data.tasks[0]

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
      <header className="rise flex items-end justify-between">
        <div>
          <p className="text-[11px] uppercase tracking-[0.2em]" style={{ color: 'var(--ink-3)' }}>Tasks</p>
          <h1 className="type-title mt-1" style={{ color: 'var(--ink)' }}>
            What you&apos;re working on
          </h1>
        </div>
        <div className="flex items-center gap-4">
          <p className="text-[12px]" style={{ color: 'var(--ink-3)' }}>
            <span className="font-mono tnum">{touched}</span> touched today
          </p>
          <div className="flex flex-col items-end gap-1">
            <button
              onClick={handleSync}
              disabled={syncing}
              className="flex items-center gap-1.5 text-[12px] px-3 py-1.5 rounded-md transition-colors"
              style={{
                color: syncing ? 'var(--ink-4)' : syncError ? '#e53e3e' : 'var(--ink-3)',
                background: 'var(--surface)',
                border: `1px solid ${syncError ? '#e53e3e' : 'var(--rule)'}`,
                cursor: syncing ? 'not-allowed' : 'pointer',
              }}
              title={lastSynced ? `Last synced ${lastSynced.toLocaleTimeString()}` : 'Sync tasks from Jira / Linear / GitHub'}
            >
              <span style={{ display: 'inline-block', animation: syncing ? 'spin 1s linear infinite' : 'none' }}>
                {syncError ? '⚠' : '↻'}
              </span>
              {syncing ? 'Syncing…' : syncError ? 'Sync failed' : 'Sync'}
            </button>
            {syncError && (
              <p className="text-[11px] max-w-[280px] text-right" style={{ color: '#e53e3e' }}>
                {syncError}
              </p>
            )}
          </div>
          <button
            onClick={() => setShowIntegrations(s => !s)}
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

      {showIntegrations ? (
        <div>
          <button
            onClick={() => setShowIntegrations(false)}
            className="flex items-center gap-1.5 text-[12px] mb-5"
            style={{ color: 'var(--ink-3)', cursor: 'pointer', background: 'none', border: 'none', padding: 0 }}
          >
            ← Back to tasks
          </button>
          <ConnectTrackers integrations={integrations} onDisconnect={fetchIntegrations} />
        </div>
      ) : (
        <>
          {showProviderTabs && (
            <div className="flex items-center gap-1">
              <ProviderTab id="all" active={providerFilter === 'all'} onClick={() => setProviderFilter('all')} />
              {presentProviders.map(p => (
                <ProviderTab key={p} id={p} active={providerFilter === p} onClick={() => setProviderFilter(p)} />
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
                      <TaskRow key={t.key} task={t} selected={t.key === selected} onSelect={() => setSelected(t.key)} epicColor={color} showProvider={showProviderTabs} />
                    ))}
                  </div>
                )
              })}
            </div>

            {sel && (
              <div className="lg:sticky lg:top-8">
                <TaskDetail task={sel} sessions={todaySessions.filter(s => s.task_key === sel.key)} />
              </div>
            )}
          </div>
        </>
      )}
    </div>
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

function TaskRow({ task, selected, onSelect, epicColor: eColor, showProvider }: { task: TaskSummary; selected: boolean; onSelect: () => void; epicColor?: string; showProvider?: boolean }) {
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
        <span className="ml-auto font-mono tnum text-[12px]" style={{ color: task.today_s > 0 ? 'var(--ink)' : 'var(--ink-4)' }}>
          {task.today_s > 0 ? fmtDur(task.today_s) : '—'}
        </span>
      </div>
      <p className="text-[13px] mt-1.5 truncate" style={{ color: 'var(--ink)' }}>{task.title}</p>
    </button>
  )
}

function TaskDetail({ task, sessions }: { task: TaskSummary; sessions: TodayResponse['sessions'] }) {
  const sortedSessions = [...sessions].sort((a, b) => a.started_at.localeCompare(b.started_at))
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

// ── Connect-a-tracker onboarding (empty state) ───────────────────────────────
// Shown when no tasks are on the board. Tells the user which trackers are wired
// up and, on click, expands the exact steps to connect one. Deliberately quiet:
// a single status line per tracker, details only when asked for.

type TrackerId = 'jira' | 'linear' | 'github' | 'trello' | 'azure_devops'

const TRACKERS: Array<{
  id: TrackerId
  name: string
  glyph: string
  color: string
  // Browser-OAuth trackers use `oauth`; static-token trackers use the token* fields.
  oauth?: { command: string; hint: string }
  tokenHint?: string
  tokenUrl?: string
  env?: string
  note?: string
}> = [
  {
    id: 'jira',
    name: 'Jira',
    glyph: 'Ji',
    color: '#2684FF',
    oauth: {
      command: 'meridian oauth-login jira',
      hint: 'Connect with your browser — no API token to create.',
    },
    tokenHint: 'Go to Atlassian account security and create an API token with "All Jira" or "Read" scope.',
    tokenUrl: 'https://id.atlassian.com/manage-profile/security/api-tokens',
    env: 'JIRA_BASE_URL=https://yourorg.atlassian.net\nJIRA_EMAIL=you@yourorg.com\nJIRA_API_TOKEN=ATATT3x…',
  },
  {
    id: 'linear',
    name: 'Linear',
    glyph: 'Li',
    color: '#5E6AD2',
    tokenHint: 'Create a personal API key (Linear → Settings → Account → Security & access).',
    tokenUrl: 'https://linear.app/settings/account/security',
    env: 'LINEAR_API_KEY=lin_api_your_key',
  },
  {
    id: 'github',
    name: 'GitHub',
    glyph: 'Gh',
    color: '#24292F',
    tokenHint: 'Easiest: run meridian setup — it pulls your token from the gh CLI (no PAT) and adds the read:project scope. Or create a classic PAT with repo, read:org, read:project scopes.',
    tokenUrl: 'https://github.com/settings/tokens/new',
    env: 'GITHUB_TOKEN=ghp_your_token\nGITHUB_PROJECT_IDS=PVT_your_project_id',
    note: 'GITHUB_PROJECT_IDS is a comma-separated list of GitHub Projects v2 node IDs. meridian setup lists your projects to pick from, or find them with: gh api graphql -f query=\'{ viewer { projectsV2(first:10){nodes{id title}} } }\'',
  },
  {
    id: 'trello',
    name: 'Trello',
    glyph: 'Tr',
    color: '#0052CC',
    oauth: {
      command: 'meridian oauth-login trello',
      hint: 'Connect with your browser — no API token to create.',
    },
  },
  {
    id: 'azure_devops',
    name: 'Azure DevOps',
    glyph: 'Az',
    color: '#0078D4',
    tokenHint: 'Open your Azure DevOps project in the browser and copy the URL from the address bar (e.g. https://dev.azure.com/myorg/MyProject). Then go to User settings → Personal access tokens → New token and create a token with Work Items → Read & write scope.',
    tokenUrl: 'https://dev.azure.com',
    env: 'AZURE_DEVOPS_URL=https://dev.azure.com/your-org/your-project\nAZURE_DEVOPS_PAT=your-pat-here',
    note: 'AZURE_DEVOPS_URL works for all URL shapes — paste exactly what is in your browser. Legacy visualstudio.com URLs and on-premises servers are supported too.',
  },
]


function ConnectTrackers({ integrations, onDisconnect }: { integrations: IntegrationsResponse | null; onDisconnect?: () => void }) {
  const [open, setOpen] = useState<TrackerId | null>(null)
  const [disconnecting, setDisconnecting] = useState<TrackerId | null>(null)
  const anyConnected = !!integrations && (integrations.jira || integrations.linear || integrations.github || integrations.trello || integrations.azure_devops)

  const handleDisconnect = (id: TrackerId) => {
    setDisconnecting(id)
    fetch(`/api/integrations?provider=${id}`, { method: 'DELETE' })
      .then(() => { onDisconnect?.(); setOpen(null) })
      .catch(() => {})
      .finally(() => setDisconnecting(null))
  }

  return (
    <div className="max-w-[560px]">
      <p className="text-[12px] mt-1" style={{ color: 'var(--ink-3)' }}>
        {anyConnected
          ? 'Manage your tracker connections below.'
          : 'Connect a tracker and Meridian maps your captured work to its tasks.'}
      </p>

      <div className="mt-5 rounded-xl border overflow-hidden" style={{ borderColor: 'var(--rule)' }}>
        {TRACKERS.map((t, i) => {
          const connected = !!integrations?.[t.id]
          const syncError = integrations?.sync_errors?.[t.id]
          const isOpen = open === t.id
          return (
            <div key={t.id} className={i > 0 ? 'rule-t' : ''} style={{ borderTopColor: 'var(--rule)' }}>
              <button
                onClick={() => setOpen(isOpen ? null : t.id)}
                className="w-full flex items-center gap-3 px-4 py-3 text-left transition-colors"
                style={{ background: isOpen ? 'var(--surface-2)' : 'var(--surface)', cursor: 'pointer' }}
              >
                <span
                  className="inline-flex items-center justify-center rounded-md font-mono shrink-0"
                  style={{ width: 22, height: 22, background: t.color + '1A', color: t.color, fontSize: 10, fontWeight: 600 }}
                >
                  {t.glyph}
                </span>
                <span className="text-[13px]" style={{ color: 'var(--ink)' }}>{t.name}</span>
                {connected ? (
                  <span className="ml-auto inline-flex items-center gap-1.5 text-[11px]" style={{ color: syncError ? '#d97706' : 'var(--ink-2)' }}>
                    <span className="inline-block w-1.5 h-1.5 rounded-full" style={{ background: syncError ? '#d97706' : 'var(--success)' }} />
                    {syncError ? 'Sync error' : 'Connected'}
                    <span className="inline-block transition-transform" style={{ transform: isOpen ? 'rotate(90deg)' : 'none', color: 'var(--ink-4)' }}>›</span>
                  </span>
                ) : (
                  <span className="ml-auto inline-flex items-center gap-2 text-[11px]" style={{ color: 'var(--ink-3)' }}>
                    Not connected
                    <span className="inline-block transition-transform" style={{ transform: isOpen ? 'rotate(90deg)' : 'none', color: 'var(--ink-4)' }}>›</span>
                  </span>
                )}
              </button>
              {isOpen && connected && (
                <ConnectedPanel
                  tracker={t}
                  syncError={syncError}
                  disconnecting={disconnecting === t.id}
                  onDisconnect={() => handleDisconnect(t.id)}
                />
              )}
              {isOpen && !connected && <TrackerSetup tracker={t} />}
            </div>
          )
        })}
      </div>
    </div>
  )
}

function ConnectedPanel({
  tracker, syncError, disconnecting, onDisconnect,
}: {
  tracker: (typeof TRACKERS)[number]
  syncError?: string
  disconnecting: boolean
  onDisconnect: () => void
}) {
  const [reauthorizing, setReauthorizing] = useState(false)
  const cleanError = syncError
    ? syncError.replace(/^permission_error: |^sync_error: /, '')
    : null

  return (
    <div className="px-4 pb-4 pt-2" style={{ background: 'var(--surface-2)' }}>
      {cleanError && !reauthorizing && (
        <div className="mb-3 rounded-md px-3 py-2" style={{ background: '#fef3c7', border: '1px solid #fcd34d' }}>
          <p className="text-[12px] leading-relaxed" style={{ color: '#92400e' }}>
            <strong>Sync failed:</strong> {cleanError}
          </p>
          <button
            onClick={() => setReauthorizing(true)}
            className="mt-2 text-[11px] px-3 py-1 rounded-md"
            style={{ background: '#92400e', color: '#fff', cursor: 'pointer' }}
          >
            Fix: Re-authorize {tracker.name}
          </button>
        </div>
      )}
      {reauthorizing && (
        <div className="mb-3">
          <p className="text-[12px] mb-2" style={{ color: 'var(--ink-2)' }}>
            Re-authorize {tracker.name}:
          </p>
          <TrackerSetup tracker={tracker} reauthorize />
          <button onClick={() => setReauthorizing(false)} className="mt-2 text-[11px]" style={{ color: 'var(--ink-4)', cursor: 'pointer' }}>
            Cancel
          </button>
        </div>
      )}
      {!reauthorizing && (
        <>
          <p className="text-[12px] leading-relaxed mb-3" style={{ color: 'var(--ink-3)' }}>
            Disconnect removes the stored credentials. The daemon reloads automatically.
          </p>
          <button
            onClick={onDisconnect}
            disabled={disconnecting}
            className="text-[12px] px-3 py-1.5 rounded-md transition-opacity"
            style={{ color: '#e53e3e', border: '1px solid #e53e3e', opacity: disconnecting ? 0.5 : 1, cursor: disconnecting ? 'not-allowed' : 'pointer', background: 'transparent' }}
          >
            {disconnecting ? 'Disconnecting…' : `Disconnect ${tracker.name}`}
          </button>
        </>
      )}
    </div>
  )
}

function TrackerSetup({ tracker, reauthorize }: { tracker: (typeof TRACKERS)[number]; reauthorize?: boolean }) {
  // Jira supports both OAuth and API token — show a chooser (or force API token on reauth)
  const [jiraMode, setJiraMode] = useState<'oauth' | 'token'>(reauthorize ? 'token' : 'oauth')
  if (tracker.id === 'jira') {
    return (
      <div style={{ background: 'var(--surface-2)' }}>
        {/* Mode toggle */}
        <div className="px-4 pt-2 pb-1 flex gap-2">
          <button
            onClick={() => setJiraMode('oauth')}
            className="text-[11px] px-3 py-1 rounded-md"
            style={{ background: jiraMode === 'oauth' ? 'var(--accent)' : 'var(--tint)', color: jiraMode === 'oauth' ? '#fff' : 'var(--ink-3)', cursor: 'pointer' }}
          >Browser OAuth</button>
          <button
            onClick={() => setJiraMode('token')}
            className="text-[11px] px-3 py-1 rounded-md"
            style={{ background: jiraMode === 'token' ? 'var(--accent)' : 'var(--tint)', color: jiraMode === 'token' ? '#fff' : 'var(--ink-3)', cursor: 'pointer' }}
          >API Token</button>
        </div>
        {jiraMode === 'oauth' ? <OAuthSetup tracker={tracker} /> : <TokenSetup tracker={tracker} />}
      </div>
    )
  }
  if (tracker.oauth) return <OAuthSetup tracker={tracker} />
  if (tracker.id === 'azure_devops') return <AzureDevOpsSetup tracker={tracker} />
  return <TokenSetup tracker={tracker} />
}

function AzureDevOpsSetup({ tracker }: { tracker: (typeof TRACKERS)[number] }) {
  const [pat, setPat] = useState('')
  const [orgs, setOrgs] = useState<string[] | null>(null)
  const [selectedOrg, setSelectedOrg] = useState('')
  const [projects, setProjects] = useState<string[] | null>(null)
  const [selectedProject, setSelectedProject] = useState('')
  const [loading, setLoading] = useState<'orgs' | 'projects' | null>(null)
  const [error, setError] = useState<string | null>(null)

  const lookupOrgs = async () => {
    if (!pat.trim()) return
    setLoading('orgs')
    setError(null)
    setOrgs(null)
    setSelectedOrg('')
    setProjects(null)
    setSelectedProject('')
    try {
      const r = await fetch('/api/integrations/azure-devops/discover', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ pat: pat.trim() }),
      })
      const json = await r.json()
      if (!r.ok) { setError(json.error ?? 'Failed to fetch organisations'); return }
      setOrgs(json.orgs ?? [])
      if ((json.orgs ?? []).length === 1) setSelectedOrg(json.orgs[0])
    } catch {
      setError('Network error — check your connection')
    } finally {
      setLoading(null)
    }
  }

  const lookupProjects = async (org: string) => {
    if (!org) return
    setLoading('projects')
    setError(null)
    setProjects(null)
    setSelectedProject('')
    try {
      const r = await fetch('/api/integrations/azure-devops/discover', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ pat: pat.trim(), org }),
      })
      const json = await r.json()
      if (!r.ok) { setError(json.error ?? 'Failed to fetch projects'); return }
      setProjects(json.projects ?? [])
      if ((json.projects ?? []).length === 1) setSelectedProject(json.projects[0])
    } catch {
      setError('Network error — check your connection')
    } finally {
      setLoading(null)
    }
  }

  const handleOrgChange = (org: string) => {
    setSelectedOrg(org)
    if (org) lookupProjects(org)
  }

  const envBlock = selectedOrg && selectedProject
    ? `AZURE_DEVOPS_URL=https://dev.azure.com/${selectedOrg}/${selectedProject}\nAZURE_DEVOPS_PAT=${pat.trim()}`
    : tracker.env ?? ''

  return (
    <div className="px-4 pb-4 pt-1" style={{ background: 'var(--surface-2)' }}>
      <ol className="space-y-3">
        <li className="flex gap-3">
          <StepNum n={1} />
          <div className="min-w-0 flex-1">
            <p className="text-[12px] leading-relaxed" style={{ color: 'var(--ink-2)' }}>
              Go to User settings → Personal access tokens → New token. Set scope to{' '}
              <strong>Work Items → Read &amp; write</strong>.{' '}
              <a href="https://dev.azure.com" target="_blank" rel="noopener noreferrer" style={{ color: 'var(--accent)' }}>
                Open ↗
              </a>
            </p>
            <div className="mt-2 flex gap-2">
              <input
                type="password"
                value={pat}
                onChange={e => setPat(e.target.value)}
                onKeyDown={e => e.key === 'Enter' && lookupOrgs()}
                placeholder="Paste your PAT here"
                className="flex-1 font-mono text-[11px] px-2 py-1.5 rounded-md border"
                style={{ color: 'var(--ink)', background: 'var(--surface)', borderColor: 'var(--rule)', outline: 'none' }}
              />
              <button
                onClick={lookupOrgs}
                disabled={!pat.trim() || loading === 'orgs'}
                className="text-[11px] px-3 py-1.5 rounded-md transition-opacity shrink-0"
                style={{
                  background: 'var(--accent)', color: '#fff',
                  opacity: (!pat.trim() || loading === 'orgs') ? 0.5 : 1,
                  cursor: (!pat.trim() || loading === 'orgs') ? 'not-allowed' : 'pointer',
                }}
              >
                {loading === 'orgs' ? 'Looking up…' : 'Look up orgs'}
              </button>
            </div>
            {error && (
              <p className="mt-1.5 text-[11px]" style={{ color: '#e53e3e' }}>{error}</p>
            )}
          </div>
        </li>

        {orgs !== null && (
          <li className="flex gap-3">
            <StepNum n={2} />
            <div className="min-w-0 flex-1">
              <p className="text-[12px] mb-1.5" style={{ color: 'var(--ink-2)' }}>
                {orgs.length === 0 ? 'No organisations found for this PAT.' : 'Choose your organisation:'}
              </p>
              {orgs.length > 0 && (
                <select
                  value={selectedOrg}
                  onChange={e => handleOrgChange(e.target.value)}
                  className="w-full text-[12px] px-2 py-1.5 rounded-md border"
                  style={{ color: 'var(--ink)', background: 'var(--surface)', borderColor: 'var(--rule)' }}
                >
                  <option value="">— select org —</option>
                  {orgs.map(o => <option key={o} value={o}>{o}</option>)}
                </select>
              )}
            </div>
          </li>
        )}

        {projects !== null && selectedOrg && (
          <li className="flex gap-3">
            <StepNum n={3} />
            <div className="min-w-0 flex-1">
              <p className="text-[12px] mb-1.5" style={{ color: 'var(--ink-2)' }}>
                {projects.length === 0 ? 'No projects found in this organisation.' : 'Choose your project:'}
              </p>
              {loading === 'projects' && (
                <p className="text-[11px]" style={{ color: 'var(--ink-3)' }}>Loading projects…</p>
              )}
              {projects.length > 0 && (
                <select
                  value={selectedProject}
                  onChange={e => setSelectedProject(e.target.value)}
                  className="w-full text-[12px] px-2 py-1.5 rounded-md border"
                  style={{ color: 'var(--ink)', background: 'var(--surface)', borderColor: 'var(--rule)' }}
                >
                  <option value="">— select project —</option>
                  {projects.map(p => <option key={p} value={p}>{p}</option>)}
                </select>
              )}
            </div>
          </li>
        )}

        <li className="flex gap-3">
          <StepNum n={selectedOrg && selectedProject ? (projects !== null ? 4 : 3) : (orgs !== null ? 3 : 2)} />
          <div className="min-w-0 flex-1">
            <p className="text-[12px] leading-relaxed" style={{ color: 'var(--ink-2)' }}>
              Run <CodeChip text="meridian config edit" /> and add:
            </p>
            <CopyBlock text={envBlock} />
          </div>
        </li>

        <li className="flex gap-3">
          <StepNum n={selectedOrg && selectedProject ? (projects !== null ? 5 : 4) : (orgs !== null ? 4 : 3)} />
          <p className="text-[12px] leading-relaxed" style={{ color: 'var(--ink-2)' }}>
            Save, then run <CodeChip text="meridian restart" />. Your tasks appear here within a minute.
          </p>
        </li>
      </ol>
    </div>
  )
}

function OAuthSetup({ tracker }: { tracker: (typeof TRACKERS)[number] }) {
  const [status, setStatus] = useState<'idle' | 'waiting' | 'done' | 'error'>('idle')
  const [error, setError] = useState<string | null>(null)

  const startOAuth = async () => {
    setStatus('waiting')
    setError(null)
    try {
      const r = await fetch(`/api/auth/oauth/start?provider=${tracker.id}`, { method: 'POST' })
      if (!r.ok) { const b = await r.json(); setError(b.error ?? 'Failed to start'); setStatus('error'); return }
      // Poll until connected (up to 3 minutes)
      const deadline = Date.now() + 180_000
      const poll = setInterval(async () => {
        if (Date.now() > deadline) { clearInterval(poll); setStatus('error'); setError('Timed out — try again'); return }
        const ir = await fetch('/api/integrations').catch(() => null)
        if (!ir?.ok) return
        const data = await ir.json()
        if (data[tracker.id]) { clearInterval(poll); setStatus('done') }
      }, 2_000)
    } catch (e) {
      setError(String(e)); setStatus('error')
    }
  }

  return (
    <div className="px-4 pb-4 pt-2" style={{ background: 'var(--surface-2)' }}>
      {status === 'idle' && (
        <div className="space-y-3">
          <p className="text-[12px] leading-relaxed" style={{ color: 'var(--ink-2)' }}>
            {tracker.oauth?.hint}
          </p>
          <button
            onClick={startOAuth}
            className="text-[12px] px-4 py-2 rounded-md font-medium transition-opacity"
            style={{ background: 'var(--accent)', color: '#fff', cursor: 'pointer' }}
          >
            Connect {tracker.name} →
          </button>
        </div>
      )}
      {status === 'waiting' && (
        <div className="space-y-2">
          <p className="text-[12px]" style={{ color: 'var(--ink-2)' }}>
            Your browser should have opened. Authorize the app, then come back here.
          </p>
          <p className="text-[11px]" style={{ color: 'var(--ink-4)' }}>Waiting for authorization…</p>
        </div>
      )}
      {status === 'done' && (
        <p className="text-[12px]" style={{ color: 'var(--success)' }}>✓ Connected! Your tasks will appear shortly.</p>
      )}
      {status === 'error' && (
        <div className="space-y-2">
          <p className="text-[12px]" style={{ color: '#e53e3e' }}>{error ?? 'OAuth failed.'}</p>
          <button onClick={() => setStatus('idle')} className="text-[11px]" style={{ color: 'var(--accent)', cursor: 'pointer' }}>Try again</button>
        </div>
      )}
    </div>
  )
}

function TokenSetup({ tracker }: { tracker: (typeof TRACKERS)[number] }) {
  const [connected, setConnected] = useState(false)
  const [checking, setChecking] = useState(false)

  const checkConnection = async () => {
    setChecking(true)
    try {
      const r = await fetch('/api/integrations')
      if (r.ok) {
        const data = await r.json()
        setConnected(!!data[tracker.id])
      }
    } catch { /* ignore */ }
    finally { setChecking(false) }
  }

  return (
    <div className="px-4 pb-4 pt-1" style={{ background: 'var(--surface-2)' }}>
      <ol className="space-y-3">
        <li className="flex gap-3">
          <StepNum n={1} />
          <div className="min-w-0 flex-1">
            <p className="text-[12px] leading-relaxed" style={{ color: 'var(--ink-2)' }}>
              {tracker.tokenHint}{' '}
              {tracker.tokenUrl && (
                <a href={tracker.tokenUrl} target="_blank" rel="noopener noreferrer" style={{ color: 'var(--accent)' }}>
                  Open ↗
                </a>
              )}
            </p>
          </div>
        </li>
        <li className="flex gap-3">
          <StepNum n={2} />
          <div className="min-w-0 flex-1">
            <p className="text-[12px] leading-relaxed" style={{ color: 'var(--ink-2)' }}>
              Run <CodeChip text="meridian config edit" /> and add:
            </p>
            {tracker.env && <CopyBlock text={tracker.env} />}
          </div>
        </li>
        <li className="flex gap-3">
          <StepNum n={3} />
          <div className="min-w-0 flex-1">
            <p className="text-[12px] leading-relaxed" style={{ color: 'var(--ink-2)' }}>
              Save, then run <CodeChip text="meridian restart" />
            </p>
            <button
              onClick={checkConnection}
              disabled={checking}
              className="mt-2 text-[11px] px-3 py-1.5 rounded-md transition-opacity"
              style={{
                background: 'var(--tint)',
                color: connected ? 'var(--success)' : 'var(--ink-2)',
                border: `1px solid ${connected ? 'var(--success)' : 'var(--rule)'}`,
                opacity: checking ? 0.6 : 1,
                cursor: checking ? 'not-allowed' : 'pointer',
              }}
            >
              {checking ? 'Checking…' : connected ? '✓ Connected!' : 'Check connection'}
            </button>
          </div>
        </li>
        {tracker.note && (
          <li className="flex gap-3">
            <span className="shrink-0 w-[18px]" />
            <p className="text-[11px] leading-relaxed" style={{ color: 'var(--ink-4)' }}>{tracker.note}</p>
          </li>
        )}
      </ol>
    </div>
  )
}

function StepNum({ n }: { n: number }) {
  return (
    <span
      className="shrink-0 inline-flex items-center justify-center rounded-full font-mono text-[10px] tnum"
      style={{ width: 18, height: 18, color: 'var(--ink-3)', background: 'var(--tint)', border: '1px solid var(--rule-2)' }}
    >
      {n}
    </span>
  )
}

function CodeChip({ text }: { text: string }) {
  return (
    <code className="font-mono text-[11px] px-1.5 py-px rounded-[4px]" style={{ color: 'var(--ink)', background: 'var(--tint)', borderBottom: '1px solid var(--rule-2)' }}>
      {text}
    </code>
  )
}

function CopyBlock({ text }: { text: string }) {
  const [copied, setCopied] = useState(false)
  const copy = () => {
    navigator.clipboard?.writeText(text).then(() => {
      setCopied(true)
      setTimeout(() => setCopied(false), 1500)
    }).catch(() => {})
  }
  return (
    <div className="relative mt-2 rounded-lg border overflow-hidden" style={{ borderColor: 'var(--rule)', background: 'var(--surface)' }}>
      <button
        onClick={copy}
        className="absolute top-2 right-2 text-[10px] uppercase tracking-[0.1em] px-2 py-1 rounded-md"
        style={{ color: copied ? 'var(--success)' : 'var(--ink-3)', background: 'var(--tint)' }}
      >
        {copied ? 'Copied' : 'Copy'}
      </button>
      <pre className="font-mono text-[11px] leading-relaxed p-3 pr-16 overflow-x-auto whitespace-pre" style={{ color: 'var(--ink-2)' }}>
        {text}
      </pre>
    </div>
  )
}

// keep this export for compat
export { AppGlyph }
