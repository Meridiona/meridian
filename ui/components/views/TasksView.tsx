// meridian — normalises screenpipe activity into structured app sessions
'use client'

import { useEffect, useState } from 'react'
import { fmtDur, fmtClock, AppGlyph, CatDot, TaskKey, StatusPill, SegBar, SectionHead, Card, CATS } from '@/components/atoms'
import type { TaskSummary, TasksResponse } from '@/app/api/tasks/route'
import type { TodayResponse } from '@/app/api/today/route'
import type { IntegrationsResponse } from '@/app/api/integrations/route'

export default function TasksView({ focusKey }: { focusKey?: string | null }) {
  const [data, setData] = useState<TasksResponse | null>(null)
  const [todaySessions, setTodaySessions] = useState<TodayResponse['sessions']>([])
  const [integrations, setIntegrations] = useState<IntegrationsResponse | null>(null)
  const [selected, setSelected] = useState<string | null>(focusKey ?? null)
  const [refreshing, setRefreshing] = useState(false)

  async function handleRefresh() {
    setRefreshing(true)
    try {
      const [tasksRes, todayRes, intRes] = await Promise.all([
        fetch('/api/tasks').then(r => r.json()),
        fetch('/api/today').then(r => r.json()),
        fetch('/api/integrations').then(r => r.json()),
      ])
      setData(tasksRes)
      setTodaySessions(todayRes.sessions ?? [])
      setIntegrations(intRes)
    } catch {
      /* ignore */
    } finally {
      setRefreshing(false)
    }
  }

  useEffect(() => {
    fetch('/api/tasks').then(r => r.json()).then((d: TasksResponse) => {
      setData(d)
      if (!selected && d.tasks.length > 0) {
        const first = d.tasks.find(t => t.today_s > 0) ?? d.tasks[0]
        setSelected(first.key)
      }
    }).catch(() => {})
    fetch('/api/today').then(r => r.json()).then((d: TodayResponse) => {
      setTodaySessions(d.sessions ?? [])
    }).catch(() => {})
    fetch('/api/integrations').then(r => r.json()).then((d: IntegrationsResponse) => {
      setIntegrations(d)
    }).catch(() => {})
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
        <ConnectTrackers integrations={integrations} />
      </div>
    )
  }

  const sel = data.tasks.find(t => t.key === selected) ?? data.tasks[0]
  const touched = data.tasks.filter(t => t.today_s > 0).length

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
            onClick={handleRefresh}
            disabled={refreshing}
            className="text-[12px] px-3 py-1.5 rounded-md transition-colors"
            style={{
              color: refreshing ? 'var(--ink-4)' : 'var(--ink-3)',
              background: 'var(--surface)',
              border: '1px solid var(--rule)',
            }}
          >
            {refreshing ? 'Syncing…' : '↻ Sync'}
          </button>
        </div>
      </header>

      <div className="grid grid-cols-1 lg:grid-cols-[minmax(0,300px)_minmax(0,1fr)] gap-8">
        {/* Task list */}
        <div className="space-y-px rule rounded-xl overflow-hidden border" style={{ borderColor: 'var(--rule)' }}>
          {data.tasks.map(t => (
            <TaskRow key={t.key} task={t} selected={t.key === selected} onSelect={() => setSelected(t.key)} />
          ))}
        </div>

        {/* Task detail */}
        {sel && <TaskDetail task={sel} sessions={todaySessions.filter(s => s.task_key === sel.key)} />}
      </div>
    </div>
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

type TrackerId = 'jira' | 'linear' | 'github'

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
]

function ConnectTrackers({ integrations }: { integrations: IntegrationsResponse | null }) {
  const [open, setOpen] = useState<TrackerId | null>(null)
  const anyConnected = !!integrations && (integrations.jira || integrations.linear || integrations.github)

  return (
    <div className="max-w-[560px]">
      <p className="text-[14px]" style={{ color: 'var(--ink-2)' }}>
        {anyConnected ? 'No tasks synced yet.' : 'No tracker connected yet.'}
      </p>
      <p className="text-[12px] mt-1" style={{ color: 'var(--ink-3)' }}>
        {anyConnected
          ? 'Tasks appear here once your tracker has issues assigned to you. Connect another below.'
          : 'Connect a tracker and Meridian maps your captured work to its tasks.'}
      </p>

      <div className="mt-5 rounded-xl border overflow-hidden" style={{ borderColor: 'var(--rule)' }}>
        {TRACKERS.map((t, i) => {
          const connected = !!integrations?.[t.id]
          const isOpen = open === t.id
          return (
            <div key={t.id} className={i > 0 ? 'rule-t' : ''} style={{ borderTopColor: 'var(--rule)' }}>
              <button
                onClick={() => setOpen(isOpen ? null : t.id)}
                disabled={connected}
                className="w-full flex items-center gap-3 px-4 py-3 text-left transition-colors"
                style={{ background: isOpen ? 'var(--surface-2)' : 'var(--surface)', cursor: connected ? 'default' : 'pointer' }}
              >
                <span
                  className="inline-flex items-center justify-center rounded-md font-mono shrink-0"
                  style={{ width: 22, height: 22, background: t.color + '1A', color: t.color, fontSize: 10, fontWeight: 600 }}
                >
                  {t.glyph}
                </span>
                <span className="text-[13px]" style={{ color: 'var(--ink)' }}>{t.name}</span>
                {connected ? (
                  <span className="ml-auto inline-flex items-center gap-1.5 text-[11px]" style={{ color: 'var(--ink-2)' }}>
                    <span className="inline-block w-1.5 h-1.5 rounded-full" style={{ background: 'var(--success)' }} />
                    Connected
                  </span>
                ) : (
                  <span className="ml-auto inline-flex items-center gap-2 text-[11px]" style={{ color: 'var(--ink-3)' }}>
                    Not connected
                    <span className="inline-block transition-transform" style={{ transform: isOpen ? 'rotate(90deg)' : 'none', color: 'var(--ink-4)' }}>›</span>
                  </span>
                )}
              </button>
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
