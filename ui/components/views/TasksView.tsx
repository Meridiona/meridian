// meridian — normalises screenpipe activity into structured app sessions
'use client'

import { useEffect, useState } from 'react'
import { fmtDur, fmtClock, AppGlyph, CatDot, TaskKey, StatusPill, SegBar, SectionHead, Card, CATS, PROVIDER_META } from '@/components/atoms'
import type { TaskSummary, TasksResponse } from '@/app/api/tasks/route'
import type { TodayResponse } from '@/app/api/today/route'
import type { IntegrationsResponse } from '@/app/api/integrations/route'

const TASKS_POLL_INTERVAL_MS = 60_000

export default function TasksView({ focusKey }: { focusKey?: string | null }) {
  const [data, setData] = useState<TasksResponse | null>(null)
  const [todaySessions, setTodaySessions] = useState<TodayResponse['sessions']>([])
  const [integrations, setIntegrations] = useState<IntegrationsResponse | null>(null)
  const [selected, setSelected] = useState<string | null>(focusKey ?? null)
  const [syncing, setSyncing] = useState(false)
  const [lastSynced, setLastSynced] = useState<Date | null>(null)
  const [providerFilter, setProviderFilter] = useState<string>('all')
  const [showIntegrations, setShowIntegrations] = useState(false)

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
    fetch('/api/tasks/sync', { method: 'POST' })
      .then(() => fetchTasks())
      .catch(() => {})
      .finally(() => {
        setSyncing(false)
        setLastSynced(new Date())
      })
  }

  useEffect(() => {
    fetchTasks()
    fetch('/api/today').then(r => r.json()).then((d: TodayResponse) => {
      setTodaySessions(d.sessions ?? [])
    }).catch(() => {})
    fetchIntegrations()

    const timer = setInterval(fetchTasks, TASKS_POLL_INTERVAL_MS)
    return () => clearInterval(timer)
  }, []) // eslint-disable-line react-hooks/exhaustive-deps

  if (!data) {
    return (
      <div className="space-y-8">
        <header className="rise">
          <p className="text-[11px] uppercase tracking-[0.2em]" style={{ color: 'var(--ink-3)' }}>Tasks</p>
          <h1 className="font-serif text-[56px] leading-[1] tracking-tight mt-1" style={{ color: 'var(--ink)' }}>What you&apos;re working on</h1>
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
          <h1 className="font-serif text-[56px] leading-[1] tracking-tight mt-1" style={{ color: 'var(--ink)' }}>What you&apos;re working on</h1>
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

  // Group tasks by provider for the "All" sectioned view.
  const tasksByProvider: Record<string, TaskSummary[]> = {}
  for (const t of visibleTasks) {
    ;(tasksByProvider[t.provider] ??= []).push(t)
  }

  return (
    <div className="space-y-8">
      <header className="rise flex items-end justify-between">
        <div>
          <p className="text-[11px] uppercase tracking-[0.2em]" style={{ color: 'var(--ink-3)' }}>Tasks</p>
          <h1 className="font-serif text-[56px] leading-[1] tracking-tight mt-1" style={{ color: 'var(--ink)' }}>
            What you&apos;re working on
          </h1>
        </div>
        <div className="flex items-center gap-4">
          <p className="text-[12px]" style={{ color: 'var(--ink-3)' }}>
            <span className="font-mono tnum">{touched}</span> touched today
            <span className="mx-2">·</span>
            <span className="font-mono tnum">{data.tasks.length}</span> on board
          </p>
          <button
            onClick={handleSync}
            disabled={syncing}
            className="flex items-center gap-1.5 text-[12px] px-3 py-1.5 rounded-md transition-colors"
            style={{
              color: syncing ? 'var(--ink-4)' : 'var(--ink-3)',
              background: 'var(--surface)',
              border: '1px solid var(--rule)',
              cursor: syncing ? 'default' : 'pointer',
            }}
            title={lastSynced ? `Last synced ${lastSynced.toLocaleTimeString()}` : 'Sync tasks from Jira / Linear / GitHub'}
          >
            <span style={{ display: 'inline-block', animation: syncing ? 'spin 1s linear infinite' : 'none' }}>↻</span>
            {syncing ? 'Syncing…' : 'Sync'}
          </button>
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
        <ConnectTrackers integrations={integrations} onDisconnect={fetchIntegrations} />
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

          <div className="grid grid-cols-1 lg:grid-cols-[minmax(0,300px)_minmax(0,1fr)] gap-8">
            <div className="rounded-xl overflow-hidden border" style={{ borderColor: 'var(--rule)' }}>
              {providerFilter === 'all' && showProviderTabs
                ? presentProviders.map((provider, pi) => {
                    const group = tasksByProvider[provider] ?? []
                    const meta = PROVIDER_META[provider]
                    return (
                      <div key={provider}>
                        <div
                          className="flex items-center gap-2 px-4 py-2"
                          style={{ background: 'var(--surface-2)', borderTop: pi > 0 ? '1px solid var(--rule)' : undefined }}
                        >
                          <span
                            className="inline-flex items-center justify-center rounded shrink-0 font-mono"
                            style={{ width: 16, height: 16, fontSize: 9, fontWeight: 700, background: (meta?.color ?? '#888') + '1A', color: meta?.color ?? '#888' }}
                          >
                            {meta?.glyph ?? provider[0].toUpperCase()}
                          </span>
                          <span className="text-[10px] uppercase tracking-[0.15em]" style={{ color: 'var(--ink-3)' }}>
                            {meta?.label ?? provider}
                          </span>
                          <span className="ml-auto font-mono tnum text-[10px]" style={{ color: 'var(--ink-4)' }}>{group.length}</span>
                        </div>
                        {group.map(t => (
                          <TaskRow key={t.key} task={t} selected={t.key === selected} onSelect={() => setSelected(t.key)} />
                        ))}
                      </div>
                    )
                  })
                : visibleTasks.map(t => (
                    <TaskRow key={t.key} task={t} selected={t.key === selected} onSelect={() => setSelected(t.key)} />
                  ))
              }
            </div>

            {sel && <TaskDetail task={sel} sessions={todaySessions.filter(s => s.task_key === sel.key)} />}
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

function TaskRow({ task, selected, onSelect }: { task: TaskSummary; selected: boolean; onSelect: () => void }) {
  const segs = Object.entries(task.cats).map(([cat, value]) => ({ cat, value }))
  return (
    <button onClick={onSelect}
      className="w-full text-left px-4 py-3 transition-colors"
      style={{
        background: selected ? 'var(--surface-2)' : 'var(--surface)',
        borderLeft: selected ? '2px solid var(--accent)' : '2px solid transparent',
      }}>
      <div className="flex items-center gap-3">
        <TaskKey keyId={task.key} />
        <StatusPill status={task.status} />
        <span className="ml-auto font-mono tnum text-[12px]" style={{ color: task.today_s > 0 ? 'var(--ink)' : 'var(--ink-4)' }}>
          {task.today_s > 0 ? fmtDur(task.today_s) : '—'}
        </span>
      </div>
      <p className="text-[13px] mt-1.5 truncate" style={{ color: 'var(--ink)' }}>{task.title}</p>
      <div className="mt-1.5">
        <SegBar
          segments={segs.length ? segs : [{ value: 1, color: 'var(--rule-2)' }]}
          height={2}
        />
      </div>
    </button>
  )
}

function TaskDetail({ task, sessions }: { task: TaskSummary; sessions: TodayResponse['sessions'] }) {
  const sortedSessions = [...sessions].sort((a, b) => a.started_at.localeCompare(b.started_at))

  return (
    <div className="space-y-7 min-w-0">
      <div>
        <div className="flex items-center gap-3 mb-3">
          <TaskKey keyId={task.key} big />
          <StatusPill status={task.status} />
          <span className="text-[11px]" style={{ color: 'var(--ink-3)' }}>{task.provider}</span>
          {task.url && (
            <a href={task.url} target="_blank" rel="noopener noreferrer"
              className="ml-auto text-[12px]" style={{ color: 'var(--ink-3)' }}>
              Open ↗
            </a>
          )}
        </div>
        <h2 className="font-serif text-[36px] leading-[1.1] tracking-tight" style={{ color: 'var(--ink)' }}>
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
                <div className="px-4 pb-4 pt-2" style={{ background: 'var(--surface-2)' }}>
                  {syncError && (
                    <div className="mb-3 rounded-md px-3 py-2 text-[12px] leading-relaxed" style={{ background: '#fef3c7', color: '#92400e', border: '1px solid #fcd34d' }}>
                      <strong>Sync failed:</strong>{' '}
                      {syncError.startsWith('permission_error: ')
                        ? syncError.slice('permission_error: '.length)
                        : syncError.startsWith('sync_error: ')
                          ? syncError.slice('sync_error: '.length)
                          : syncError}
                    </div>
                  )}
                  <p className="text-[12px] leading-relaxed mb-3" style={{ color: 'var(--ink-3)' }}>
                    After disconnecting, run <CodeChip text="meridian restart" /> for the change to take effect.
                  </p>
                  <button
                    onClick={() => handleDisconnect(t.id)}
                    disabled={disconnecting === t.id}
                    className="text-[12px] px-3 py-1.5 rounded-md transition-opacity"
                    style={{ color: '#e53e3e', border: '1px solid #e53e3e', opacity: disconnecting === t.id ? 0.5 : 1, cursor: disconnecting === t.id ? 'default' : 'pointer', background: 'transparent' }}
                  >
                    {disconnecting === t.id ? 'Disconnecting…' : `Disconnect ${t.name}`}
                  </button>
                </div>
              )}
              {isOpen && !connected && <TrackerSetup tracker={t} />}
            </div>
          )
        })}
      </div>
    </div>
  )
}

function TrackerSetup({ tracker }: { tracker: (typeof TRACKERS)[number] }) {
  // Browser-OAuth trackers (Jira) get the one-command flow; everyone else gets
  // the get-a-token / paste-env / restart flow.
  return tracker.oauth ? <OAuthSetup oauth={tracker.oauth} /> : <TokenSetup tracker={tracker} />
}

function OAuthSetup({ oauth }: { oauth: { command: string; hint: string } }) {
  return (
    <div className="px-4 pb-4 pt-1" style={{ background: 'var(--surface-2)' }}>
      <ol className="space-y-3">
        <li className="flex gap-3">
          <StepNum n={1} />
          <div className="min-w-0 flex-1">
            <p className="text-[12px] leading-relaxed" style={{ color: 'var(--ink-2)' }}>
              {oauth.hint} In a terminal, run:
            </p>
            <CopyBlock text={oauth.command} />
            <p className="mt-2 text-[11px] leading-relaxed" style={{ color: 'var(--ink-3)' }}>
              Your browser opens — pick your site and click Accept.
            </p>
          </div>
        </li>
        <li className="flex gap-3">
          <StepNum n={2} />
          <p className="text-[12px] leading-relaxed" style={{ color: 'var(--ink-2)' }}>
            Run <CodeChip text="meridian restart" />. Your tasks appear here within a minute.
          </p>
        </li>
      </ol>
    </div>
  )
}

function TokenSetup({ tracker }: { tracker: (typeof TRACKERS)[number] }) {
  return (
    <div className="px-4 pb-4 pt-1" style={{ background: 'var(--surface-2)' }}>
      <ol className="space-y-3">
        <li className="flex gap-3">
          <StepNum n={1} />
          <p className="text-[12px] leading-relaxed" style={{ color: 'var(--ink-2)' }}>
            {tracker.tokenHint}{' '}
            <a href={tracker.tokenUrl} target="_blank" rel="noopener noreferrer" style={{ color: 'var(--accent)' }}>
              Open ↗
            </a>
          </p>
        </li>
        <li className="flex gap-3">
          <StepNum n={2} />
          <div className="min-w-0 flex-1">
            <p className="text-[12px] leading-relaxed" style={{ color: 'var(--ink-2)' }}>
              In a terminal, run <CodeChip text="meridian config edit" /> and add:
            </p>
            <CopyBlock text={tracker.env ?? ''} />
            {tracker.note && (
              <p className="mt-2 text-[11px] leading-relaxed" style={{ color: 'var(--ink-3)' }}>
                {tracker.note}
              </p>
            )}
          </div>
        </li>
        <li className="flex gap-3">
          <StepNum n={3} />
          <p className="text-[12px] leading-relaxed" style={{ color: 'var(--ink-2)' }}>
            Save, then run <CodeChip text="meridian restart" />. Your tasks appear here within a minute.
          </p>
        </li>
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
