//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The ONE shared "connect your trackers" surface — used by BOTH the timeline
// app (TasksPanel) and the first-run wizard (setup). Driven entirely by the
// metadata in `@/lib/integrations`, so adding/changing a provider happens in one place.
//
// Connect flows map to tray commands:
//   - Browser OAuth  → `start_oauth` + poll `get_oauth_status` (jira/trello run
//     the loopback-redirect flow in-process; github uses the device flow in-process).
//   - Token / PAT    → `save_integration_token` (writes .env + reloads daemon).
//                      NO terminal step — the old "run `meridian config edit`"
//                      instructions are gone.
//   - Azure DevOps   → `discover_azure_devops` (PAT → org → project) then
//                      `save_integration_token`.

import { useEffect, useRef, useState, type ReactNode } from 'react'
import { invoke, load, mutate, openExternal } from '@/lib/bridge'
import type { IntegrationsResponse, TaskSummary, TasksResponse } from '@/lib/api-types'
import { TRACKERS } from '@/lib/integrations'
import type { Tracker, TokenField } from '@/lib/integrations'
import { ProviderGlyph, fmtDur } from '@/components/atoms'
import { SelectableRow } from '@/components/ui/SelectableRow'
import { syncTasks } from '@/lib/taskSync'
import {
  useConnectStore, clearProviderNotice,
  oauthStore, startOAuth, cancelOAuth, restartOAuth, setOAuthApiKey, resetOAuthIfSettled,
  azureStore, azureLookupOrgs, azureSelectOrg, azureSubmitManualOrg, azureConnect,
  setAzurePat, setAzureSelectedProject, setAzureManualOrg, resetAzureIfSettled,
  githubPickerStore, githubEnsureLoaded, githubToggle, githubSave, resetGithubPickerIfSaved,
  jiraPickerStore, jiraEnsureLoaded, jiraToggle, jiraSave, resetJiraPickerIfSaved,
} from '@/components/integrationConnectStore'
import type { GithubProject, JiraProject } from '@/components/integrationConnectStore'

// ── Main list ─────────────────────────────────────────────────────────────────
export default function ConnectTrackers({
  integrations, onChanged, compact, onDecline,
}: {
  integrations: IntegrationsResponse | null
  onChanged?: () => void
  compact?: boolean
  /** Only meaningful when this list is rendered as the onboarding gate
   *  (`IntegrationsSection`'s `gate` prop) — see `RequestToolForm`'s doc for
   *  why submitting "I don't see my tool" also fires this. */
  onDecline?: () => void
}) {
  const [open, setOpen] = useState<string | null>(null)
  const [disconnecting, setDisconnecting] = useState<string | null>(null)
  const anyConnected = !!integrations && TRACKERS.some((t) => integrations[t.id])
  // "I don't see my tool" is a side channel, not a connect flow: it never
  // gates onboarding and always coexists with the tracker list above it.
  const [requestingTool, setRequestingTool] = useState(false)

  const handleDisconnect = (id: string) => {
    setDisconnecting(id)
    // disconnect_integration (Rust) in the app, /api/integrations DELETE in a browser.
    mutate(`/api/integrations?provider=${id}`, 'disconnect_integration', { provider: id }, 'DELETE')
      .then(() => { onChanged?.(); setOpen(null) })
      .catch(() => {})
      .finally(() => setDisconnecting(null))
  }

  return (
    <div style={{ maxWidth: compact ? '100%' : 560 }}>
      {!compact && (
        <p className="text-[12px] mt-1" style={{ color: 'var(--t-faint)' }}>
          {anyConnected
            ? 'Manage your tracker connections below.'
            : 'Connect a tracker and Meridian maps your captured work to its tasks.'}
        </p>
      )}

      <div className={compact ? 'rounded-xl border overflow-hidden' : 'mt-5 rounded-xl border overflow-hidden'} style={{ borderColor: 'var(--t-hair)' }}>
        {TRACKERS.map((t, i) => {
          const connected = !!integrations?.[t.id]
          const syncError = integrations?.sync_errors?.[t.id]
          const isOpen = open === t.id
          // Gate NEW connections only. An already-connected tracker keeps its
          // full row so a user can still manage or disconnect it - see the
          // `comingSoon` doc in `@/lib/integrations`.
          const locked = !!t.comingSoon && !connected
          return (
            <div key={t.id} style={{ borderTop: i > 0 ? '1px solid var(--t-hair)' : undefined }}>
              <button
                onClick={() => { if (!locked) setOpen(isOpen ? null : t.id) }}
                disabled={locked}
                aria-disabled={locked}
                className="w-full flex items-center gap-3 px-4 py-3 text-left transition-colors"
                style={{ background: isOpen ? 'var(--t-box)' : 'var(--t-card)', cursor: locked ? 'default' : 'pointer', opacity: locked ? 0.55 : 1 }}
              >
                <ProviderGlyph provider={t.id} size={22} />
                <span className="flex flex-col min-w-0">
                  <span className="text-[13px]" style={{ color: 'var(--t-title)' }}>{t.name}</span>
                  {compact && !connected && <span className="text-[11px] truncate" style={{ color: 'var(--t-faint-2)' }}>{t.blurb}</span>}
                </span>
                {locked ? (
                  <span className="ml-auto text-[11px]" style={{ color: 'var(--t-faint-2)' }}>Coming soon</span>
                ) : connected ? (
                  <span className="ml-auto inline-flex items-center gap-1.5 text-[11px]" style={{ color: syncError ? 'var(--status-warning-dot)' : 'var(--t-muted)' }}>
                    <span className="inline-block w-1.5 h-1.5 rounded-full" style={{ background: syncError ? 'var(--status-warning-dot)' : 'var(--color-state-approved)' }} />
                    {syncError ? 'Sync error' : 'Connected'}
                    <span className="inline-block transition-transform" style={{ transform: isOpen ? 'rotate(90deg)' : 'none', color: 'var(--t-faint-2)' }}>›</span>
                  </span>
                ) : (
                  <span className="ml-auto inline-flex items-center gap-2 text-[11px]" style={{ color: 'var(--t-faint)' }}>
                    Connect
                    <span className="inline-block transition-transform" style={{ transform: isOpen ? 'rotate(90deg)' : 'none', color: 'var(--t-faint-2)' }}>›</span>
                  </span>
                )}
              </button>
              {isOpen && connected && (
                <ConnectedPanel tracker={t} syncError={syncError} disconnecting={disconnecting === t.id}
                  onDisconnect={() => handleDisconnect(t.id)} onChanged={onChanged}
                  githubProjectsSelected={integrations?.github_projects_selected}
                  jiraProjectsSelected={integrations?.jira_projects_selected} />
              )}
              {isOpen && !connected && <TrackerSetup tracker={t} onSuccess={onChanged} />}
            </div>
          )
        })}
        <div style={{ borderTop: '1px solid var(--t-hair)' }}>
          {requestingTool ? (
            <RequestToolForm onDone={() => setRequestingTool(false)} onDecline={onDecline} />
          ) : (
            <button
              onClick={() => setRequestingTool(true)}
              className="w-full flex items-center gap-3 px-4 py-3 text-left transition-colors"
              style={{ background: 'var(--t-card)', cursor: 'pointer' }}>
              <span className="text-[13px]" style={{ color: 'var(--t-faint)' }}>I don&apos;t see my tool</span>
            </button>
          )}
        </div>
      </div>
    </div>
  )
}

// ── "I don't see my tool" (request_pm_tool — settings mirror + PostHog) ─────
// A side channel outside the gate: submitting there neither blocks nor
// advances anything, since there is nothing to advance. `request_pm_tool`
// (Rust) does two independent, best-effort things with the free text -
// mirrors it into settings.json and fires a `pm_tool_requested` PostHog
// event - see that command's doc for why both.
//
// UNDER the onboarding gate, though, naming a tool IS the same answer as
// clicking "I don't use a project tool": the user does not have one of the
// five we support, so a successful submit also fires `onDecline` (passed
// through from `IntegrationsSection`'s own decline handler) to release the
// tour's lock exactly the way the explicit skip button does - the walkthrough
// then narrates the same "let's write this one by hand instead" beat rather
// than leaving the user stranded on the connect screen after telling us they
// have no match here. `onDecline` is `undefined` outside the gate, so this is
// a no-op there.
function RequestToolForm({ onDone, onDecline }: { onDone: () => void; onDecline?: () => void }) {
  const [value, setValue] = useState('')
  const [submitting, setSubmitting] = useState(false)
  const [submitted, setSubmitted] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const submit = () => {
    const tool_name = value.trim()
    if (!tool_name || submitting) return
    setSubmitting(true); setError(null)
    // A single top-level scalar param (`tool_name`), not a `body:` struct -
    // reached via `invoke` directly, per `mutate`'s calling convention (see
    // `__tests__/mutate-body-contract.test.ts` and `save_account_email`'s
    // call sites for the same shape).
    invoke('request_pm_tool', { toolName: tool_name })
      .then(() => { setSubmitted(true); onDecline?.() })
      .catch((e) => setError(typeof e === 'string' ? e : e instanceof Error ? e.message : 'Could not send'))
      .finally(() => setSubmitting(false))
  }

  if (submitted) {
    return (
      <div className="px-4 py-3" style={{ background: 'var(--t-box)' }}>
        <p className="text-[12px]" style={{ color: 'var(--color-state-approved)' }}>
          Thanks - we&apos;ll let you know if we add it.
        </p>
      </div>
    )
  }

  return (
    <div className="px-4 py-3 space-y-2" style={{ background: 'var(--t-box)' }}>
      <label className="block">
        <span className="text-[11px]" style={{ color: 'var(--t-faint)' }}>What tool do you use?</span>
        <input
          type="text"
          value={value}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => { if (e.key === 'Enter') submit() }}
          placeholder="e.g. Shortcut, Height, Basecamp"
          autoFocus
          className="mt-1 w-full text-[12px] px-2 py-1.5 rounded-md border"
          style={{ color: 'var(--t-title)', background: 'var(--t-card)', borderColor: 'var(--t-hair)', outline: 'none' }}
        />
      </label>
      {error && <p className="text-[11px]" style={{ color: 'var(--status-error-dot)' }}>{error}</p>}
      <div className="flex gap-2">
        <button onClick={submit} disabled={!value.trim() || submitting}
          className="text-[12px] px-3 py-1.5 rounded-md font-medium transition-opacity"
          style={{ background: 'var(--btn-primary-bg)', color: '#fff', opacity: (!value.trim() || submitting) ? 0.5 : 1, cursor: (!value.trim() || submitting) ? 'not-allowed' : 'pointer' }}>
          {submitting ? 'Sending…' : 'Send'}
        </button>
        <button onClick={onDone} className="text-[12px] px-3 py-1.5" style={{ color: 'var(--t-faint-2)', cursor: 'pointer' }}>
          Cancel
        </button>
      </div>
    </div>
  )
}

// ── Connected (manage / disconnect / re-authorize) ───────────────────────────
function ConnectedPanel({
  tracker, syncError, disconnecting, onDisconnect, onChanged, githubProjectsSelected, jiraProjectsSelected,
}: {
  tracker: Tracker; syncError?: string; disconnecting: boolean; onDisconnect: () => void; onChanged?: () => void
  /** Only meaningful for tracker.id === 'github' — undefined for every other tracker. */
  githubProjectsSelected?: boolean
  /** Only meaningful for tracker.id === 'jira' — undefined for every other tracker. */
  jiraProjectsSelected?: boolean
}) {
  const [reauthorizing, setReauthorizing] = useState(false)
  const [pickingProjects, setPickingProjects] = useState(false)
  const [showTasks, setShowTasks] = useState(false)
  const cleanError = syncError ? syncError.replace(/^permission_error: |^sync_error: /, '') : null
  // GitHub's token alone doesn't sync anything — a Projects v2 board must be
  // selected too (discover_github_projects → save_integration_token). This is
  // exactly the gap a token connected outside the OAuth-connect picker (or an
  // account connected before this picker existed) is stuck in.
  const needsGithubProjects = tracker.id === 'github' && githubProjectsSelected === false
  // Jira's analogue — a connection (OAuth OR API token) alone doesn't sync
  // anything either; a project must be picked too (discover_jira_projects →
  // save_integration_token).
  const needsJiraProjects = tracker.id === 'jira' && jiraProjectsSelected === false

  return (
    <div className="px-4 pb-4 pt-2" style={{ background: 'var(--t-box)' }}>
      {cleanError && !reauthorizing && !pickingProjects && (
        <div className="mb-3 rounded-md px-3 py-2" style={{ background: 'var(--status-warning-bg)', border: '1px solid var(--status-warning-border)' }}>
          <p className="text-[12px] leading-relaxed" style={{ color: 'var(--status-warning-text)' }}>
            <strong>Sync failed:</strong> {cleanError}
          </p>
          <button onClick={() => setReauthorizing(true)} className="mt-2 text-[11px] px-3 py-1 rounded-md"
            style={{ background: 'var(--status-warning-text)', color: '#fff', cursor: 'pointer' }}>
            Fix: Reconnect {tracker.name}
          </button>
        </div>
      )}
      {needsGithubProjects && !reauthorizing && !pickingProjects && (
        <div className="mb-3 rounded-md px-3 py-2" style={{ background: 'var(--status-info-bg)', border: '1px solid var(--status-info-border)' }}>
          <p className="text-[12px] leading-relaxed" style={{ color: 'var(--status-info-text)' }}>
            No GitHub Projects selected - tasks won&apos;t sync yet.
          </p>
          <button onClick={() => setPickingProjects(true)} className="mt-2 text-[11px] px-3 py-1 rounded-md"
            style={{ background: 'var(--status-info-text)', color: '#fff', cursor: 'pointer' }}>
            Select Projects
          </button>
        </div>
      )}
      {needsJiraProjects && !reauthorizing && !pickingProjects && (
        <div className="mb-3 rounded-md px-3 py-2" style={{ background: 'var(--status-info-bg)', border: '1px solid var(--status-info-border)' }}>
          <p className="text-[12px] leading-relaxed" style={{ color: 'var(--status-info-text)' }}>
            No Jira projects selected - tasks won&apos;t sync yet.
          </p>
          <button onClick={() => setPickingProjects(true)} className="mt-2 text-[11px] px-3 py-1 rounded-md"
            style={{ background: 'var(--status-info-text)', color: '#fff', cursor: 'pointer' }}>
            Select Projects
          </button>
        </div>
      )}
      {pickingProjects ? (
        <div className="mb-1">
          {/* Warm start here too - picking projects on an already-connected tracker is
              the same "there are tickets to pull now" moment as a fresh connect, and
              this is the path a user reaches from the "no projects selected" nudge. */}
          {tracker.id === 'github'
            ? <GitHubProjectPicker onSuccess={() => { void syncTasks(); setPickingProjects(false); onChanged?.() }} />
            : <JiraProjectPicker onSuccess={() => { void syncTasks(); setPickingProjects(false); onChanged?.() }} />}
          <button onClick={() => setPickingProjects(false)} className="mt-2 text-[11px]" style={{ color: 'var(--ink-4)', cursor: 'pointer' }}>Cancel</button>
        </div>
      ) : reauthorizing ? (
        <div className="mb-1">
          <p className="text-[12px] mb-2" style={{ color: 'var(--t-muted)' }}>Reconnect {tracker.name}:</p>
          <TrackerSetup tracker={tracker} onSuccess={() => { setReauthorizing(false); onChanged?.() }} />
          <button onClick={() => setReauthorizing(false)} className="mt-2 text-[11px]" style={{ color: 'var(--t-faint-2)', cursor: 'pointer' }}>Cancel</button>
        </div>
      ) : (
        <>
          <ProviderTasks tracker={tracker} expanded={showTasks} onToggle={() => setShowTasks((s) => !s)} onSynced={onChanged} />
          <p className="text-[12px] leading-relaxed mb-3 mt-4" style={{ color: 'var(--t-faint)' }}>
            Disconnect removes the stored credentials. The daemon reloads automatically.
          </p>
          <button onClick={onDisconnect} disabled={disconnecting} className="text-[12px] px-3 py-1.5 rounded-md transition-opacity"
            style={{ color: 'var(--status-error-dot)', border: '1px solid var(--status-error-dot)', opacity: disconnecting ? 0.5 : 1, cursor: disconnecting ? 'not-allowed' : 'pointer', background: 'transparent' }}>
            {disconnecting ? 'Disconnecting…' : `Disconnect ${tracker.name}`}
          </button>
        </>
      )}
    </div>
  )
}

// ── View tasks + sync (per connected tracker) ────────────────────────────────
// The "Task list" surface, folded into the tracker card. "View tasks" loads the
// board via `get_tasks` and filters to THIS provider (TaskSummary.provider ===
// tracker.id); "Sync now" runs `sync_tasks` (= `meridian tasks-sync`, all
// providers — there is no per-provider sync command) then reloads the list and
// the parent integrations state. Both tray commands already back the old
// TasksPanel, so this reuses the exact data path.
function ProviderTasks({ tracker, expanded, onToggle, onSynced }: {
  tracker: Tracker; expanded: boolean; onToggle: () => void; onSynced?: () => void
}) {
  const [tasks, setTasks] = useState<TaskSummary[] | null>(null)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [syncing, setSyncing] = useState(false)
  const [syncError, setSyncError] = useState<string | null>(null)
  // The lazy load (on expand) and the sync-triggered reload can overlap — a
  // fetch generation counter so a slower, older response can never overwrite
  // a fresher one that already landed.
  const fetchGen = useRef(0)

  const fetchTasks = () => {
    const gen = ++fetchGen.current
    setLoadError(null)
    load<TasksResponse>('/api/tasks', 'get_tasks')
      .then((d) => { if (gen === fetchGen.current) setTasks(d.tasks.filter((t) => t.provider === tracker.id)) })
      .catch((e) => { if (gen === fetchGen.current) setLoadError(typeof e === 'string' ? e : e instanceof Error ? e.message : 'Could not load tasks') })
  }

  // Load lazily — only once the user opens the list, and only the first time.
  useEffect(() => { if (expanded && tasks === null && loadError === null) fetchTasks() /* eslint-disable-next-line react-hooks/exhaustive-deps */ }, [expanded])

  const handleSync = () => {
    if (syncing) return
    setSyncing(true); setSyncError(null)
    mutate('/api/tasks/sync', 'sync_tasks', {})
      .then(() => { fetchTasks(); onSynced?.() })
      .catch((e) => setSyncError(typeof e === 'string' ? e : e instanceof Error ? e.message : 'Sync failed'))
      .finally(() => setSyncing(false))
  }

  // Active (on-board) tasks first, then by time-touched-today — mirrors the board.
  const sorted = tasks ? [...tasks].sort((a, b) => Number(b.on_board) - Number(a.on_board) || b.today_s - a.today_s) : null
  const activeCount = tasks?.filter((t) => t.on_board).length ?? 0

  return (
    <div className="mb-1">
      <div className="flex items-center gap-2">
        <button onClick={onToggle}
          className="text-[12px] px-3 py-1.5 rounded-md inline-flex items-center gap-1.5"
          style={{ background: 'var(--t-ctrl, var(--t-card))', border: '1px solid var(--t-ctrl-border, var(--t-hair))', color: 'var(--t-muted)', cursor: 'pointer' }}>
          <span className="inline-block transition-transform" style={{ transform: expanded ? 'rotate(90deg)' : 'none', color: 'var(--t-faint-2)' }}>›</span>
          {expanded ? 'Hide tasks' : 'View tasks'}
          {expanded && tasks !== null && (
            <span className="text-[11px]" style={{ color: 'var(--t-faint-2)' }}>({activeCount})</span>
          )}
        </button>
        <button onClick={handleSync} disabled={syncing}
          className="text-[12px] px-3 py-1.5 rounded-md inline-flex items-center gap-1.5"
          style={{ background: 'var(--btn-primary-bg)', color: '#fff', border: 'none', opacity: syncing ? 0.6 : 1, cursor: syncing ? 'not-allowed' : 'pointer' }}
          title="Pull the latest tasks from your trackers">
          <span style={{ display: 'inline-block', animation: syncing ? 'spin 1s linear infinite' : 'none' }}>↻</span>
          {syncing ? 'Syncing…' : 'Sync now'}
        </button>
      </div>

      {syncError && <p className="text-[11px] mt-2" style={{ color: 'var(--status-error-dot)' }}>{syncError}</p>}

      {expanded && (
        <div className="mt-3 rounded-lg overflow-hidden" style={{ border: '1px solid var(--t-hair)', background: 'var(--t-card)' }}>
          {loadError ? (
            <div className="px-3 py-3">
              <p className="text-[12px]" style={{ color: 'var(--status-error-dot)' }}>{loadError}</p>
              <button onClick={fetchTasks} className="text-[11px] mt-2" style={{ color: 'var(--t-accent)', cursor: 'pointer' }}>Retry</button>
            </div>
          ) : sorted === null ? (
            <p className="text-[12px] px-3 py-3" style={{ color: 'var(--t-faint)' }}>Loading tasks…</p>
          ) : sorted.length === 0 ? (
            <p className="text-[12px] px-3 py-3 leading-relaxed" style={{ color: 'var(--t-muted)' }}>
              No tasks assigned to you on {tracker.name}. Make sure issues are assigned to your account, then hit Sync now.
            </p>
          ) : (
            <div className="max-h-64 overflow-y-auto">
              {sorted.map((t, i) => (
                <div key={t.key} className="flex items-center gap-2.5 px-3 py-2"
                  style={{ borderTop: i > 0 ? '1px solid var(--t-hair)' : undefined, opacity: t.on_board ? 1 : 0.55 }}>
                  <a href={t.url} onClick={(e) => { e.preventDefault(); if (t.url) openExternal(t.url) }}
                    className="text-[11px] font-mono px-1.5 py-0.5 rounded shrink-0"
                    style={{ background: 'var(--t-box)', color: 'var(--t-accent)', cursor: t.url ? 'pointer' : 'default' }}
                    title={t.url ? `Open ${t.key} ↗` : t.key}>
                    {t.key}
                  </a>
                  <span className="text-[12px] truncate flex-1" style={{ color: 'var(--t-title)' }}>{t.title}</span>
                  {!t.on_board && <span className="text-[10px] shrink-0" style={{ color: 'var(--t-faint-2)' }}>done</span>}
                  <span className="text-[11px] font-mono shrink-0" style={{ color: t.today_s > 0 ? 'var(--t-muted)' : 'var(--t-faint-2)' }}>
                    {t.today_s > 0 ? fmtDur(t.today_s) : '—'}
                  </span>
                </div>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  )
}

// ── Flow picker ───────────────────────────────────────────────────────────────
function TrackerSetup({ tracker, onSuccess }: { tracker: Tracker; onSuccess?: () => void }) {
  // Providers that offer BOTH OAuth and a token get a mode toggle.
  const [mode, setMode] = useState<'oauth' | 'token'>(tracker.oauth ? 'oauth' : 'token')

  // WARM START. Every connect path in this component funnels its completion through
  // here, so this is the one place that knows "a tracker just became usable" for all
  // of them - and it is several seconds earlier than the planner, which is where the
  // first sync used to be kicked off. Deliberately not awaited: the point is that the
  // network round trip overlaps the confirmation the user is about to read and the
  // screens they pass through next, instead of being paid in front of an empty board.
  // See `@/lib/taskSync` for why a second caller joins this rather than duplicating it.
  const done = () => { void syncTasks(); onSuccess?.() }

  if (tracker.azure) return <AzureDevOpsSetup tracker={tracker} onSuccess={done} />

  return (
    <div style={{ background: 'var(--t-box)' }}>
      {/* Narrowed inline rather than gated on `dual`. `dual` is a separately
          computed boolean, and TypeScript cannot see that it implies both fields
          are present - which is why this used to need `tracker.oauth!`. The `!`
          was correct today and silently load-bearing: change how `dual` is derived
          and the assertion keeps compiling while the guarantee is gone. */}
      {tracker.oauth && tracker.token && (
        <div className="px-4 pt-2 pb-1 flex gap-2">
          <ModeTab label={tracker.oauth.label} active={mode === 'oauth'} onClick={() => setMode('oauth')} />
          <ModeTab label={tracker.token.label} active={mode === 'token'} onClick={() => setMode('token')} />
        </div>
      )}
      {mode === 'oauth' && tracker.oauth
        ? <OAuthSetup tracker={tracker} onSuccess={done} />
        : <TokenSetup tracker={tracker} onSuccess={done} />}
    </div>
  )
}

function ModeTab({ label, active, onClick }: { label: string; active: boolean; onClick: () => void }) {
  return (
    <button onClick={onClick} className="text-[11px] px-3 py-1 rounded-md"
      style={{ background: active ? 'var(--btn-primary-bg)' : 'color-mix(in srgb, var(--t-accent) 10%, transparent)', color: active ? '#fff' : 'var(--t-faint)', cursor: 'pointer' }}>
      {label}
    </button>
  )
}

// ── Device-flow checklist step ───────────────────────────────────────────────
// One row of the GitHub device-flow checklist: a status badge (done ✓ / active
// numbered / waiting spinner / todo dimmed) + a title + the step's content.
function DeviceStep({ n, state, title, children }: {
  n: number; state: 'done' | 'active' | 'todo' | 'waiting'; title: string; children?: ReactNode
}) {
  const badge =
    state === 'done' ? (
      <span className="flex items-center justify-center w-5 h-5 rounded-full text-[11px]"
        style={{ background: 'var(--color-state-approved)', color: '#fff' }}>✓</span>
    ) : state === 'waiting' ? (
      <span className="block w-5 h-5 rounded-full animate-spin"
        style={{ border: '2px solid var(--t-hair)', borderTopColor: 'var(--t-accent)' }} />
    ) : (
      <span className="flex items-center justify-center w-5 h-5 rounded-full text-[11px] font-semibold"
        style={{
          border: `1.5px solid ${state === 'active' ? 'var(--t-accent)' : 'var(--t-hair)'}`,
          color: state === 'active' ? 'var(--t-accent)' : 'var(--t-faint-2)',
        }}>{n}</span>
    )
  return (
    <div className="flex gap-3" style={{ opacity: state === 'todo' ? 0.5 : 1, transition: 'opacity 0.2s' }}>
      <div className="shrink-0 mt-0.5">{badge}</div>
      <div className="flex-1 min-w-0">
        <p className="text-[13px] font-medium" style={{ color: 'var(--t-title)' }}>{title}</p>
        {children && <div className="mt-2">{children}</div>}
      </div>
    </div>
  )
}

// ── GitHub "Organization access" schematic ───────────────────────────────────
// A theme-aware inline SVG (not a real screenshot — CSP blocks external images
// and a screenshot would leak real orgs + go stale) illustrating GitHub's
// authorize page: one already-granted org and one with a highlighted "Grant"
// button an arrow points at, so first-timers know exactly what to click.
function GitHubGrantHint() {
  const hair = 'var(--t-hair)'
  const card = 'var(--t-card)'
  const muted = 'var(--t-faint-2)'
  const accent = 'var(--t-accent)'
  const ok = 'var(--color-state-approved)'
  return (
    <svg viewBox="0 0 280 120" role="img"
      aria-label="On GitHub's authorize page, click Grant or Request next to each organisation"
      className="w-full" style={{ maxWidth: 300, height: 'auto' }}>
      {/* card */}
      <rect x="1" y="1" width="198" height="112" rx="8" style={{ fill: card, stroke: hair }} strokeWidth="1" />
      <text x="14" y="21" style={{ fill: muted }} fontSize="9" fontWeight="600">Organization access</text>
      {/* row 1 — already granted */}
      <circle cx="21" cy="46" r="7" style={{ fill: hair }} />
      <rect x="35" y="42" width="66" height="7" rx="3.5" style={{ fill: hair }} />
      <rect x="149" y="38" width="40" height="17" rx="8.5" style={{ fill: 'none', stroke: ok }} strokeWidth="1.3" />
      <text x="169" y="50" textAnchor="middle" style={{ fill: ok }} fontSize="9" fontWeight="700">✓</text>
      {/* row 2 — needs a grant (highlighted) */}
      <circle cx="21" cy="78" r="7" style={{ fill: hair }} />
      <rect x="35" y="74" width="58" height="7" rx="3.5" style={{ fill: hair }} />
      <rect x="149" y="70" width="40" height="17" rx="8.5" style={{ fill: accent }} />
      <text x="169" y="82" textAnchor="middle" style={{ fill: '#fff' }} fontSize="8" fontWeight="700">Grant</text>
      {/* arrow from the callout to the Grant button — a straight shaft plus a
          hand-computed triangular head whose three points are all derived from
          the shaft's own direction vector, so the head stays aligned with the
          line (the old curve + freehand triangle didn't and rendered broken). */}
      <line x1="232" y1="94" x2="199" y2="83" style={{ stroke: accent }} strokeWidth="1.6" strokeLinecap="round" />
      <polygon points="191,80 198,87 201,79" style={{ fill: accent }} />
      <text x="266" y="97" textAnchor="end" style={{ fill: accent }} fontSize="9" fontWeight="600">click for</text>
      <text x="266" y="109" textAnchor="end" style={{ fill: accent }} fontSize="9" fontWeight="600">each org</text>
    </svg>
  )
}

// ── Browser OAuth (start_oauth + poll) ───────────────────────────────────────
// A thin view over `oauthStore` — all flow state and the poll loop live in the
// store (integrationConnectStore.ts) so an in-flight attempt (crucially the
// device code the user is typing at github.com) survives this panel unmounting.
function OAuthSetup({ tracker, onSuccess }: { tracker: Tracker; onSuccess?: () => void }) {
  const { status, error, deviceCode, verifyUri, apiKeyPrompt, apiKey } = useConnectStore(oauthStore, tracker.id)
  // `copied` is the momentary label flash on the copy button (reverts after 1.5s).
  // `copiedOnce` / `opened` are the PERSISTENT checklist ticks — they must not
  // revert, and they gate the next step (Step 2's Open button unlocks only after
  // a copy). All local: a 3-step checklist for one in-flight connect, not state
  // worth surviving an accordion collapse (the device code itself lives in the store).
  const [copied, setCopied] = useState(false)
  const [copiedOnce, setCopiedOnce] = useState(false)
  const [opened, setOpened] = useState(false)

  // On mount, clear a settled (done/error) attempt back to idle — but never
  // touch a 'waiting' one, which is the in-flight state we exist to preserve.
  useEffect(() => { resetOAuthIfSettled(tracker.id) }, [tracker.id])

  // Fire onSuccess when the flow completes. GitHub and Jira both defer this to
  // their Projects pickers (rendered below for status==='done'), which call
  // onSuccess once a project is actually saved. Every other provider is done
  // the moment its store exists. The store poll sets 'done' whether or not
  // this panel is mounted; if it isn't, reopening reloads integrations anyway,
  // so nothing is missed.
  useEffect(() => {
    if (status === 'done' && tracker.id !== 'github' && tracker.id !== 'jira') onSuccess?.()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [status, tracker.id])

  return (
    <div className="px-4 pb-4 pt-2" style={{ background: 'var(--t-box)' }}>
      {status === 'idle' && (
        <div className="space-y-3">
          <p className="text-[12px] leading-relaxed" style={{ color: 'var(--t-muted)' }}>{tracker.oauth?.hint}</p>
          {apiKeyPrompt && (
            <>
              <p className="text-[12px]" style={{ color: 'var(--status-warning-dot)' }}>
                A Trello API key is required.{' '}
                <a href="https://trello.com/app-key" onClick={(e) => { e.preventDefault(); openExternal('https://trello.com/app-key') }} style={{ color: 'var(--t-accent)' }}>Get it at trello.com/app-key ↗</a>
              </p>
              <Field
                field={{
                  name: 'api_key', label: 'API Key', placeholder: 'Paste your Trello API key', required: true,
                  // Trello only redirects back to origins on the key's allow-list,
                  // so without this the browser flow dead-ends after consent.
                  hint: 'On the same page, add http://127.0.0.1:9123 under Allowed Origins — Trello only redirects back to listed origins.',
                }}
                value={apiKey}
                onChange={(v) => setOAuthApiKey(tracker.id, v)}
                onEnter={() => { if (apiKey.trim()) void startOAuth(tracker) }}
                autoFocus
              />
            </>
          )}
          <button
            onClick={() => void startOAuth(tracker)}
            disabled={apiKeyPrompt && !apiKey.trim()}
            className="text-[12px] px-4 py-2 rounded-md font-medium transition-opacity"
            style={{
              background: 'var(--btn-primary-bg)', color: '#fff',
              opacity: apiKeyPrompt && !apiKey.trim() ? 0.5 : 1,
              cursor: apiKeyPrompt && !apiKey.trim() ? 'not-allowed' : 'pointer',
            }}>
            Connect {tracker.name} →
          </button>
        </div>
      )}
      {status === 'waiting' && (
        <div className="space-y-2">
          {deviceCode ? (
            // A guided 3-step checklist. Steps 1 and 2 are actions in OUR UI, so
            // they self-tick (copiedOnce / opened). Step 3 happens entirely on
            // GitHub's own authorize page — which is cross-origin in the user's
            // real browser, so we CANNOT observe per-org Grant clicks or tick them
            // as they happen; it stays a spinner until the whole flow succeeds, at
            // which point OAuthSetup swaps this view for the project picker. Do not
            // try to auto-tick per org — there is no signal for it.
            <div className="space-y-3">
              <p className="text-[13px] font-semibold" style={{ color: 'var(--t-title)' }}>
                Connect GitHub - 3 quick steps
              </p>

              <DeviceStep n={1} state={copiedOnce ? 'done' : 'active'} title="Copy your one-time code">
                <div className="flex items-center gap-2 flex-wrap">
                  <code className="font-mono text-[18px] tracking-[0.25em] px-3 py-2 rounded-md border"
                    style={{ color: 'var(--t-title)', background: 'var(--t-card)', borderColor: 'var(--t-hair)' }}>
                    {deviceCode}
                  </code>
                  <button
                    onClick={() => {
                      void navigator.clipboard?.writeText(deviceCode).catch(() => {})
                      setCopiedOnce(true); setCopied(true)
                      setTimeout(() => setCopied(false), 1500)
                    }}
                    className="text-[12px] px-3 py-2 rounded-md font-medium transition-colors"
                    style={{
                      color: '#fff',
                      background: copied ? 'var(--color-state-approved)' : 'var(--btn-primary-bg)',
                      border: 'none', cursor: 'pointer',
                    }}>
                    {copied ? '✓ Copied' : 'Copy code'}
                  </button>
                </div>
              </DeviceStep>

              <DeviceStep n={2} state={opened ? 'done' : copiedOnce ? 'active' : 'todo'} title="Open GitHub and paste the code">
                <button
                  disabled={!copiedOnce}
                  onClick={() => { openExternal(verifyUri ?? 'https://github.com/login/device'); setOpened(true) }}
                  className="text-[12px] px-3 py-2 rounded-md font-medium inline-flex items-center gap-1.5 transition-opacity"
                  style={{
                    background: 'var(--btn-primary-bg)', color: '#fff',
                    opacity: copiedOnce ? 1 : 0.45, border: 'none',
                    cursor: copiedOnce ? 'pointer' : 'not-allowed',
                  }}>
                  Open GitHub ↗
                </button>
                <p className="text-[11px] mt-1.5" style={{ color: 'var(--t-faint-2)' }}>
                  {copiedOnce
                    ? 'Opens github.com/login/device - paste the code into the box there.'
                    : 'Copy the code first, then this button opens GitHub.'}
                </p>
              </DeviceStep>

              <DeviceStep n={3} state="waiting" title="Grant each organisation, then Authorize">
                <GitHubGrantHint />
                <p className="text-[11px] mt-2 leading-relaxed" style={{ color: 'var(--t-faint-2)' }}>
                  GitHub lists your organisations separately - click <strong>Grant</strong> (or <strong>Request</strong>) next to each one you want Meridian to access, then press Authorize. Waiting for GitHub…
                </p>
              </DeviceStep>
            </div>
          ) : (
            <>
              <p className="text-[12px]" style={{ color: 'var(--t-muted)' }}>Your browser should have opened. Authorize the app, then come back here.</p>
              <p className="text-[11px]" style={{ color: 'var(--t-faint-2)' }}>Waiting for authorization…</p>
              {/* THE STUCK-USER EXITS, and there are two of them because there are
                  two ways this stalls. A rejection the provider shows on THEIR
                  consent screen ("no Jira site access") never redirects back, so
                  no error arrives to catch - but far more often the browser tab
                  simply never opened, or was closed, or landed on the wrong
                  account, and what that user wants is another go rather than a
                  way out.

                  Real buttons, not the 11px faint pseudo-link this used to be.
                  That link sat below a line reading "Waiting for authorization…"
                  and was styled like the caption above it, on the one screen
                  where the user is already unsure whether anything is happening -
                  which is exactly where an escape hatch has to look pressable.

                  No longer gated on jira/trello: this branch is only reached by
                  browser-OAuth trackers in the first place, and being stuck is
                  not provider-specific. `cancelOAuth` already does the
                  provider-specific server-side teardown internally. */}
              <div className="flex items-center gap-2 pt-0.5">
                <button onClick={() => { void restartOAuth(tracker) }}
                  className={`text-[12px] px-3 py-1.5 rounded-md transition-opacity hover:opacity-80`}
                  style={{
                    fontWeight: 700, color: 'var(--t-accent)', cursor: 'pointer',
                    background: 'color-mix(in srgb, var(--t-accent) 12%, transparent)',
                    border: '1px solid color-mix(in srgb, var(--t-accent) 30%, transparent)',
                  }}>
                  Open it again
                </button>
                <button onClick={() => cancelOAuth(tracker)}
                  className={`text-[12px] px-3 py-1.5 rounded-md transition-opacity hover:opacity-80`}
                  style={{
                    color: 'var(--t-muted)', cursor: 'pointer',
                    background: 'var(--t-ctrl)', border: '1px solid var(--t-ctrl-border)',
                  }}>
                  Cancel
                </button>
              </div>
            </>
          )}
        </div>
      )}
      {status === 'done' && (
        tracker.id === 'github'
          ? (
            // Continue the SAME checklist rather than swapping to a disconnected
            // screen: steps 1-3 collapse to plain done ticks (their interactive
            // controls no longer matter - the code was copied, GitHub was opened,
            // access was granted) and the project picker becomes Step 4, the next
            // active item in the same list.
            <div className="space-y-3">
              <DeviceStep n={1} state="done" title="Copy your one-time code" />
              <DeviceStep n={2} state="done" title="Open GitHub and paste the code" />
              <DeviceStep n={3} state="done" title="Granted organisation access on GitHub" />
              <DeviceStep n={4} state="active" title="Choose your boards">
                <GitHubProjectPicker onSuccess={onSuccess} />
              </DeviceStep>
            </div>
          )
          : tracker.id === 'jira'
            ? (
              <div className="space-y-2">
                <p className="text-[12px]" style={{ color: 'var(--color-state-approved)' }}>✓ Connected! Choose which projects to sync:</p>
                <JiraProjectPicker onSuccess={onSuccess} />
              </div>
            )
            : <p className="text-[12px]" style={{ color: 'var(--color-state-approved)' }}>✓ Connected! Your tasks will appear shortly.</p>
      )}
      {status === 'error' && (
        <div className="space-y-2">
          <p className="text-[12px]" style={{ color: 'var(--status-error-dot)' }}>{error ?? 'OAuth failed.'}</p>
          <button onClick={() => cancelOAuth(tracker)} className="text-[11px]" style={{ color: 'var(--t-accent)', cursor: 'pointer' }}>
            Try again
          </button>
        </div>
      )}
    </div>
  )
}

// ── Token / PAT (save_integration_token — writes .env, reloads daemon) ────────
function TokenSetup({ tracker, onSuccess }: { tracker: Tracker; onSuccess?: () => void }) {
  const method = tracker.token
  if (!method) return null
  const [values, setValues] = useState<Record<string, string>>({})
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [done, setDone] = useState(false)
  const [reloadWarning, setReloadWarning] = useState(false)

  const canSave = method.fields.filter((f) => f.required).every((f) => (values[f.name] ?? '').trim().length > 0)

  const save = async () => {
    if (!canSave || saving) return
    setSaving(true); setError(null)
    try {
      // save_integration_token (Rust) writes .env + reloads the daemon.
      const res = await mutate<{ ok: boolean; reloaded: boolean }>('/api/auth/token', 'save_integration_token', { provider: tracker.id, fields: values })
      setDone(true); setReloadWarning(res?.reloaded === false)
      clearProviderNotice(tracker.id)
      // GitHub defers onSuccess to its Projects picker (rendered in the done
      // branch below), which fires it once a board is actually saved. Firing it
      // now would reload integrations, flip the row to "connected", and unmount
      // this panel — tearing the picker away before the user picks. Mirrors
      // OAuthSetup's github handling. Every other provider is done on save.
      if (tracker.id !== 'github' && tracker.id !== 'jira') onSuccess?.()
    } catch (e) {
      setError(typeof e === 'string' ? e : e instanceof Error ? e.message : 'Could not save credentials')
    } finally {
      setSaving(false)
    }
  }

  if (done) {
    // GitHub/Jira: the token alone doesn't sync anything — a project must be
    // selected too. Show the same discover-and-tick picker the browser OAuth flow
    // uses (discover_github_projects / discover_jira_projects → save_integration_token).
    if (tracker.id === 'github' || tracker.id === 'jira') {
      return (
        <div className="px-4 pb-4 pt-2" style={{ background: 'var(--t-box)' }}>
          {tracker.id === 'github'
            ? <GitHubProjectPicker onSuccess={onSuccess} />
            : <JiraProjectPicker onSuccess={onSuccess} />}
          {reloadWarning && (
            <p className="text-[11px] mt-2" style={{ color: 'var(--t-faint)' }}>
              The daemon wasn&apos;t running - credentials saved, will take effect on next start.
            </p>
          )}
        </div>
      )
    }
    return (
      <div className="px-4 pb-4 pt-2" style={{ background: 'var(--t-box)' }}>
        <p className="text-[12px]" style={{ color: 'var(--color-state-approved)' }}>✓ Connected! Your tasks will appear shortly.</p>
        {reloadWarning && (
          <p className="text-[11px] mt-1" style={{ color: 'var(--t-faint)' }}>
            The daemon wasn&apos;t running — credentials saved, will take effect on next start.
          </p>
        )}
      </div>
    )
  }

  return (
    <div className="px-4 pb-4 pt-2 space-y-3" style={{ background: 'var(--t-box)' }}>
      <p className="text-[12px] leading-relaxed" style={{ color: 'var(--t-muted)' }}>
        {method.hint}
      </p>
      {method.url && (
        <button
          onClick={() => openExternal(method.url!)}
          className="text-[12px] px-3 py-2 rounded-md font-medium inline-flex items-center gap-1.5"
          style={{ background: 'var(--btn-primary-bg)', color: '#fff', border: 'none', cursor: 'pointer' }}>
          Open {tracker.name} ↗
        </button>
      )}
      {method.fields.map((f) => (
        <Field key={f.name} field={f} value={values[f.name] ?? ''}
          onChange={(v) => setValues((s) => ({ ...s, [f.name]: v }))}
          onEnter={save} />
      ))}
      {error && <p className="text-[11px]" style={{ color: 'var(--status-error-dot)' }}>{error}</p>}
      {method.note && <p className="text-[11px] leading-relaxed" style={{ color: 'var(--t-faint-2)' }}>{method.note}</p>}
      <button onClick={save} disabled={!canSave || saving} className="text-[12px] px-4 py-2 rounded-md font-medium transition-opacity"
        style={{ background: 'var(--btn-primary-bg)', color: '#fff', opacity: !canSave || saving ? 0.5 : 1, cursor: !canSave || saving ? 'not-allowed' : 'pointer' }}>
        {saving ? 'Connecting…' : `Connect ${tracker.name}`}
      </button>
    </div>
  )
}

function Field({ field, value, onChange, onEnter, autoFocus }: {
  field: TokenField; value: string; onChange: (v: string) => void; onEnter?: () => void; autoFocus?: boolean
}) {
  return (
    <label className="block">
      <span className="text-[11px]" style={{ color: 'var(--t-faint)' }}>{field.label}</span>
      <input
        type={field.password ? 'password' : 'text'}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={(e) => { if (e.key === 'Enter' && onEnter) onEnter() }}
        placeholder={field.placeholder}
        autoFocus={autoFocus}
        className="mt-1 w-full font-mono text-[11px] px-2 py-1.5 rounded-md border"
        style={{ color: 'var(--t-title)', background: 'var(--t-card)', borderColor: 'var(--t-hair)', outline: 'none' }}
      />
      {field.hint && <span className="text-[10px] leading-relaxed block mt-1" style={{ color: 'var(--t-faint-2)' }}>{field.hint}</span>}
    </label>
  )
}

// ── Azure DevOps (PAT → org → project → save) ────────────────────────────────
// A thin view over `azureStore` — the typed PAT, discovered orgs/projects, and
// the multi-step progress live in the store so closing Settings mid-discovery
// doesn't force the user to paste the PAT and re-run every lookup.
function AzureDevOpsSetup({ tracker: _tracker, onSuccess }: { tracker: Tracker; onSuccess?: () => void }) {
  const {
    pat, orgs, selectedOrg, projects, selectedProject, loading, error, done, reloadWarning, showManualOrg, manualOrg,
  } = useConnectStore(azureStore, 'azure_devops')

  // Fresh slate on mount only when the last attempt actually completed; an
  // in-progress discovery (typed PAT, fetched orgs) is preserved across unmount.
  useEffect(() => { resetAzureIfSettled() }, [])
  useEffect(() => { if (done) onSuccess?.(); /* eslint-disable-next-line react-hooks/exhaustive-deps */ }, [done])

  const lookupOrgs = () => void azureLookupOrgs()
  const connect = () => void azureConnect()

  if (done) {
    return (
      <div className="px-4 pb-4 pt-2" style={{ background: 'var(--t-box)' }}>
        <p className="text-[12px]" style={{ color: 'var(--color-state-approved)' }}>✓ Connected! Your tasks will appear shortly.</p>
        {reloadWarning && (
          <p className="text-[11px] mt-1" style={{ color: 'var(--t-faint)' }}>
            The daemon wasn&apos;t running — credentials saved, will take effect on next start.
          </p>
        )}
      </div>
    )
  }

  return (
    <div className="px-4 pb-4 pt-2 space-y-3" style={{ background: 'var(--t-box)' }}>
      <p className="text-[12px] leading-relaxed" style={{ color: 'var(--t-muted)' }}>
        In Azure DevOps go to User settings → Personal access tokens → New token, set scope to{' '}
        <strong>All accessible organizations</strong> and enable <strong>Work Items → Read &amp; write</strong>.{' '}
        <a href="https://dev.azure.com" onClick={(e) => { e.preventDefault(); openExternal('https://dev.azure.com') }} style={{ color: 'var(--t-accent)' }}>Open ↗</a>
      </p>
      <div className="flex gap-2">
        <input type="password" value={pat} onChange={(e) => setAzurePat(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && lookupOrgs()} placeholder="Paste your PAT here"
          className="flex-1 font-mono text-[11px] px-2 py-1.5 rounded-md border"
          style={{ color: 'var(--t-title)', background: 'var(--t-card)', borderColor: 'var(--t-hair)', outline: 'none' }} />
        <button onClick={lookupOrgs} disabled={!pat.trim() || loading === 'orgs'}
          className="text-[11px] px-3 py-1.5 rounded-md shrink-0"
          style={{ background: 'var(--btn-primary-bg)', color: '#fff', opacity: (!pat.trim() || loading === 'orgs') ? 0.5 : 1, cursor: (!pat.trim() || loading === 'orgs') ? 'not-allowed' : 'pointer' }}>
          {loading === 'orgs' ? 'Looking up…' : 'Look up'}
        </button>
      </div>

      {orgs !== null && (
        orgs.length === 0
          ? <p className="text-[12px]" style={{ color: 'var(--t-faint)' }}>No organisations found for this PAT.</p>
          : (
            <label className="block">
              <span className="text-[11px]" style={{ color: 'var(--t-faint)' }}>Organisation</span>
              <select value={selectedOrg} onChange={(e) => azureSelectOrg(e.target.value)}
                className="mt-1 w-full text-[12px] px-2 py-1.5 rounded-md border"
                style={{ color: 'var(--t-title)', background: 'var(--t-card)', borderColor: 'var(--t-hair)' }}>
                <option value="">— select org —</option>
                {orgs.map((o) => <option key={o} value={o}>{o}</option>)}
              </select>
            </label>
          )
      )}

      {projects !== null && selectedOrg && (
        loading === 'projects'
          ? <p className="text-[11px]" style={{ color: 'var(--t-faint)' }}>Loading projects…</p>
          : projects.length === 0
            ? <p className="text-[12px]" style={{ color: 'var(--t-faint)' }}>No projects found in this organisation.</p>
            : (
              <label className="block">
                <span className="text-[11px]" style={{ color: 'var(--t-faint)' }}>Project</span>
                <select value={selectedProject} onChange={(e) => setAzureSelectedProject(e.target.value)}
                  className="mt-1 w-full text-[12px] px-2 py-1.5 rounded-md border"
                  style={{ color: 'var(--t-title)', background: 'var(--t-card)', borderColor: 'var(--t-hair)' }}>
                  <option value="">— select project —</option>
                  {projects.map((p) => <option key={p} value={p}>{p}</option>)}
                </select>
              </label>
            )
      )}

      {error && <p className="text-[11px]" style={{ color: 'var(--status-error-dot)' }}>{error}</p>}

      {showManualOrg && !orgs && (
        <div className="space-y-1.5">
          <label htmlFor="azure-devops-org" className="text-[11px]" style={{ color: 'var(--t-faint)' }}>Enter your org name manually:</label>
          <div className="flex gap-2">
            <input
              id="azure-devops-org"
              value={manualOrg}
              onChange={(e) => setAzureManualOrg(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && azureSubmitManualOrg()}
              placeholder="e.g. my-company"
              className="flex-1 text-[11px] px-2 py-1.5 rounded-md border"
              style={{ color: 'var(--t-title)', background: 'var(--t-card)', borderColor: 'var(--t-hair)', outline: 'none' }} />
            <button
              onClick={azureSubmitManualOrg}
              disabled={!manualOrg.trim() || loading === 'projects'}
              className="text-[11px] px-3 py-1.5 rounded-md shrink-0"
              style={{ background: 'var(--btn-primary-bg)', color: '#fff', opacity: (!manualOrg.trim() || loading === 'projects') ? 0.5 : 1, cursor: (!manualOrg.trim() || loading === 'projects') ? 'not-allowed' : 'pointer' }}>
              {loading === 'projects' ? 'Looking up…' : 'Look up projects'}
            </button>
          </div>
        </div>
      )}

      {selectedOrg && selectedProject && (
        <button onClick={connect} disabled={loading === 'saving'} className="text-[12px] px-4 py-2 rounded-md font-medium transition-opacity"
          style={{ background: 'var(--btn-primary-bg)', color: '#fff', opacity: loading === 'saving' ? 0.5 : 1, cursor: loading === 'saving' ? 'not-allowed' : 'pointer' }}>
          {loading === 'saving' ? 'Connecting…' : 'Connect Azure DevOps'}
        </button>
      )}
    </div>
  )
}

// ── GitHub Projects v2 picker (discover_github_projects → save_integration_token) ──
// Runs right after a GitHub OAuth connect succeeds (see OAuthSetup's status==='done'
// branch) AND from ConnectedPanel's "no projects selected" prompt — same component,
// two entry points, since the underlying gap (token connected, no board chosen) is
// identical either way. A thin view over `githubPickerStore` so the discovered
// board list and the user's checkbox selection survive the panel unmounting.
function GitHubProjectPicker({ onSuccess }: { onSuccess?: () => void }) {
  const { projects, selected, loading, saving, loadError, saveError, reloadWarning, saved } = useConnectStore(githubPickerStore, 'github')

  // Load once (store-guarded), and clear a completed save so reopening re-discovers.
  useEffect(() => { resetGithubPickerIfSaved(); githubEnsureLoaded() }, [])
  useEffect(() => { if (saved) onSuccess?.(); /* eslint-disable-next-line react-hooks/exhaustive-deps */ }, [saved])

  if (loading) return <p className="text-[11px]" style={{ color: 'var(--t-muted)' }}>Loading your GitHub Projects…</p>

  if (loadError) return (
    <div className="space-y-2">
      <p className="text-[12px]" style={{ color: 'var(--status-error-text)' }}>{loadError}</p>
      <button onClick={() => githubEnsureLoaded()} className="text-[11px] px-3 py-1.5 rounded-md"
        style={{ color: 'var(--t-accent)', border: '1px solid var(--t-hair)', cursor: 'pointer', background: 'transparent' }}>
        Retry
      </button>
    </div>
  )

  if (!projects || projects.length === 0) {
    return (
      <p className="text-[12px] leading-relaxed" style={{ color: 'var(--t-muted)' }}>
        No GitHub Projects v2 boards found on this account.{' '}
        <a href="https://github.com/users/me/projects" target="_blank" rel="noopener noreferrer" style={{ color: 'var(--accent)' }}>Create one ↗</a>
      </p>
    )
  }

  const byOwner = projects.reduce<Record<string, GithubProject[]>>((acc, p) => {
    (acc[p.owner] ??= []).push(p)
    return acc
  }, {})

  return (
    <div className="space-y-3">
      <p className="text-[12px] leading-relaxed" style={{ color: 'var(--t-muted)' }}>
        Pick which GitHub Projects v2 boards to sync tasks from.
      </p>
      <div className="flex flex-col gap-3 max-h-56 overflow-y-auto -mx-1 px-1 py-0.5">
        {Object.entries(byOwner).map(([owner, ps]) => (
          <div key={owner} className="flex flex-col gap-1.5">
            <span className="mt-label" style={{ color: 'var(--t-faint)' }}>{owner}</span>
            {ps.map((p) => (
              <SelectableRow
                key={p.id}
                checked={selected.has(p.id)}
                onChange={() => githubToggle(p.id)}
                title={p.title}
              />
            ))}
          </div>
        ))}
      </div>
      {saveError && <p className="text-[11px]" style={{ color: 'var(--status-error-text)' }}>{saveError}</p>}
      <button onClick={() => void githubSave()} disabled={selected.size === 0 || saving} className="mt-body-sm px-4 py-2 rounded-lg transition-all"
        style={{
          fontWeight: 700, color: '#fff',
          background: 'var(--btn-primary-bg)',
          // The lift is what makes it read as the thing to press. Dropped when it
          // cannot be pressed - a glowing disabled button is a lie.
          boxShadow: (selected.size === 0 || saving) ? 'none' : '0 8px 22px -10px var(--t-accent)',
          opacity: (selected.size === 0 || saving) ? 0.45 : 1,
          cursor: (selected.size === 0 || saving) ? 'not-allowed' : 'pointer',
        }}>
        {saving ? 'Saving…' : selected.size === 0 ? 'Pick at least one' : `Sync ${selected.size} project${selected.size === 1 ? '' : 's'}`}
      </button>
      {reloadWarning && (
        <p className="text-[11px]" style={{ color: 'var(--t-faint)' }}>
          The daemon wasn&apos;t running — saved, will take effect on next start.
        </p>
      )}
    </div>
  )
}

// ── Jira project picker (discover_jira_projects → save_integration_token) ────
// Runs right after a Jira connect succeeds — OAuth (OAuthSetup's status==='done'
// branch) OR API token (TokenSetup's done branch) — AND from ConnectedPanel's
// "no projects selected" prompt. Same component, three entry points, since the
// underlying gap (connected, no project chosen) is identical across all of them.
// A thin view over `jiraPickerStore` so the discovered project list and the
// user's checkbox selection survive the panel unmounting. Flat list (no
// owner grouping — a Jira site's projects have no GitHub-org analogue).
function JiraProjectPicker({ onSuccess }: { onSuccess?: () => void }) {
  const { projects, selected, loading, saving, loadError, saveError, reloadWarning, saved } = useConnectStore(jiraPickerStore, 'jira')

  // Load once (store-guarded), and clear a completed save so reopening re-discovers.
  useEffect(() => { resetJiraPickerIfSaved(); jiraEnsureLoaded() }, [])
  useEffect(() => { if (saved) onSuccess?.(); /* eslint-disable-next-line react-hooks/exhaustive-deps */ }, [saved])

  if (loading) return <p className="text-[11px]" style={{ color: 'var(--t-muted)' }}>Loading your Jira projects…</p>

  if (loadError) return (
    <div className="space-y-2">
      <p className="text-[12px]" style={{ color: 'var(--status-error-text)' }}>{loadError}</p>
      <button onClick={() => jiraEnsureLoaded()} className="text-[11px] px-3 py-1.5 rounded-md"
        style={{ color: 'var(--t-accent)', border: '1px solid var(--t-hair)', cursor: 'pointer', background: 'transparent' }}>
        Retry
      </button>
    </div>
  )

  if (!projects || projects.length === 0) {
    return (
      <p className="text-[12px] leading-relaxed" style={{ color: 'var(--t-muted)' }}>
        No Jira projects found for this account.
      </p>
    )
  }

  return (
    <div className="space-y-3">
      <p className="text-[12px] leading-relaxed" style={{ color: 'var(--t-muted)' }}>
        Pick which Jira projects to sync tasks from.
      </p>
      {/* -mx-1 px-1: the rows carry their own border and selected fill, so they need
          a hair of room either side or the focus ring and shadow clip on the scroll
          container's edge. */}
      <div className="flex flex-col gap-1.5 max-h-56 overflow-y-auto -mx-1 px-1 py-0.5">
        {projects.map((p: JiraProject) => (
          <SelectableRow
            key={p.id}
            checked={selected.has(p.id)}
            onChange={() => jiraToggle(p.id)}
            title={p.name}
            meta={p.key}
          />
        ))}
      </div>
      {saveError && <p className="text-[11px]" style={{ color: 'var(--status-error-text)' }}>{saveError}</p>}
      <button onClick={() => void jiraSave()} disabled={selected.size === 0 || saving} className="mt-body-sm px-4 py-2 rounded-lg transition-all"
        style={{
          fontWeight: 700, color: '#fff',
          background: 'var(--btn-primary-bg)',
          // The lift is what makes it read as the thing to press. Dropped when it
          // cannot be pressed - a glowing disabled button is a lie.
          boxShadow: (selected.size === 0 || saving) ? 'none' : '0 8px 22px -10px var(--t-accent)',
          opacity: (selected.size === 0 || saving) ? 0.45 : 1,
          cursor: (selected.size === 0 || saving) ? 'not-allowed' : 'pointer',
        }}>
        {saving ? 'Saving…' : selected.size === 0 ? 'Pick at least one' : `Sync ${selected.size} project${selected.size === 1 ? '' : 's'}`}
      </button>
      {reloadWarning && (
        <p className="text-[11px]" style={{ color: 'var(--t-faint)' }}>
          The daemon wasn&apos;t running — saved, will take effect on next start.
        </p>
      )}
    </div>
  )
}
