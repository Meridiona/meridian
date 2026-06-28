//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The ONE shared "connect your trackers" surface — used by BOTH the dashboard
// (TasksView) and the first-run wizard (setup). Driven entirely by the metadata
// in `@/lib/integrations`, so adding/changing a provider happens in one place.
//
// Connect flows map to tray commands:
//   - Browser OAuth  → `start_oauth` + poll `get_oauth_status` (jira/trello run
//     in-process in the tray; github shells the gh CLI).
//   - Token / PAT    → `save_integration_token` (writes .env + reloads daemon).
//                      NO terminal step — the old "run `meridian config edit`"
//                      instructions are gone.
//   - Azure DevOps   → `discover_azure_devops` (PAT → org → project) then
//                      `save_integration_token`.

import { useEffect, useRef, useState } from 'react'
import { load, invoke, mutate } from '@/lib/bridge'
import type { IntegrationsResponse } from '@/lib/api-types'
import { TRACKERS } from '@/lib/integrations'
import type { Tracker, TokenField } from '@/lib/integrations'

const OAUTH_DEADLINE_MS = 180_000  // 3-minute browser-consent window

// Clear a provider's error notice immediately (don't wait for the next ETL poll).
// Fire-and-forget — a failure just means the banner clears on the next poll.
function clearProviderNotice(provider: string): void {
  void invoke('delete_notice', { noticeId: `pm.${provider}` }).catch(() => {})
}

// ── Main list ─────────────────────────────────────────────────────────────────
export default function ConnectTrackers({
  integrations, onChanged, compact,
}: {
  integrations: IntegrationsResponse | null
  onChanged?: () => void
  compact?: boolean
}) {
  const [open, setOpen] = useState<string | null>(null)
  const [disconnecting, setDisconnecting] = useState<string | null>(null)
  const anyConnected = !!integrations && TRACKERS.some((t) => integrations[t.id])

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
        <p className="text-[12px] mt-1" style={{ color: 'var(--ink-3)' }}>
          {anyConnected
            ? 'Manage your tracker connections below.'
            : 'Connect a tracker and Meridian maps your captured work to its tasks.'}
        </p>
      )}

      <div className={compact ? 'rounded-xl border overflow-hidden' : 'mt-5 rounded-xl border overflow-hidden'} style={{ borderColor: 'var(--rule)' }}>
        {TRACKERS.map((t, i) => {
          const connected = !!integrations?.[t.id]
          const syncError = integrations?.sync_errors?.[t.id]
          const isOpen = open === t.id
          return (
            <div key={t.id} style={{ borderTop: i > 0 ? '1px solid var(--rule)' : undefined }}>
              <button
                onClick={() => setOpen(isOpen ? null : t.id)}
                className="w-full flex items-center gap-3 px-4 py-3 text-left transition-colors"
                style={{ background: isOpen ? 'var(--surface-2)' : 'var(--surface)', cursor: 'pointer' }}
              >
                <span className="inline-flex items-center justify-center rounded-md font-mono shrink-0"
                  style={{ width: 22, height: 22, background: t.color + '1A', color: t.color, fontSize: 10, fontWeight: 600 }}>
                  {t.glyph}
                </span>
                <span className="flex flex-col min-w-0">
                  <span className="text-[13px]" style={{ color: 'var(--ink)' }}>{t.name}</span>
                  {compact && !connected && <span className="text-[11px] truncate" style={{ color: 'var(--ink-4)' }}>{t.blurb}</span>}
                </span>
                {connected ? (
                  <span className="ml-auto inline-flex items-center gap-1.5 text-[11px]" style={{ color: syncError ? '#d97706' : 'var(--ink-2)' }}>
                    <span className="inline-block w-1.5 h-1.5 rounded-full" style={{ background: syncError ? '#d97706' : 'var(--success)' }} />
                    {syncError ? 'Sync error' : 'Connected'}
                    <span className="inline-block transition-transform" style={{ transform: isOpen ? 'rotate(90deg)' : 'none', color: 'var(--ink-4)' }}>›</span>
                  </span>
                ) : (
                  <span className="ml-auto inline-flex items-center gap-2 text-[11px]" style={{ color: 'var(--ink-3)' }}>
                    Connect
                    <span className="inline-block transition-transform" style={{ transform: isOpen ? 'rotate(90deg)' : 'none', color: 'var(--ink-4)' }}>›</span>
                  </span>
                )}
              </button>
              {isOpen && connected && (
                <ConnectedPanel tracker={t} syncError={syncError} disconnecting={disconnecting === t.id}
                  onDisconnect={() => handleDisconnect(t.id)} onChanged={onChanged} />
              )}
              {isOpen && !connected && <TrackerSetup tracker={t} onSuccess={onChanged} />}
            </div>
          )
        })}
      </div>
    </div>
  )
}

// ── Connected (manage / disconnect / re-authorize) ───────────────────────────
function ConnectedPanel({
  tracker, syncError, disconnecting, onDisconnect, onChanged,
}: {
  tracker: Tracker; syncError?: string; disconnecting: boolean; onDisconnect: () => void; onChanged?: () => void
}) {
  const [reauthorizing, setReauthorizing] = useState(false)
  const cleanError = syncError ? syncError.replace(/^permission_error: |^sync_error: /, '') : null

  return (
    <div className="px-4 pb-4 pt-2" style={{ background: 'var(--surface-2)' }}>
      {cleanError && !reauthorizing && (
        <div className="mb-3 rounded-md px-3 py-2" style={{ background: '#fef3c7', border: '1px solid #fcd34d' }}>
          <p className="text-[12px] leading-relaxed" style={{ color: '#92400e' }}>
            <strong>Sync failed:</strong> {cleanError}
          </p>
          <button onClick={() => setReauthorizing(true)} className="mt-2 text-[11px] px-3 py-1 rounded-md"
            style={{ background: '#92400e', color: '#fff', cursor: 'pointer' }}>
            Fix: Reconnect {tracker.name}
          </button>
        </div>
      )}
      {reauthorizing ? (
        <div className="mb-1">
          <p className="text-[12px] mb-2" style={{ color: 'var(--ink-2)' }}>Reconnect {tracker.name}:</p>
          <TrackerSetup tracker={tracker} onSuccess={() => { setReauthorizing(false); onChanged?.() }} />
          <button onClick={() => setReauthorizing(false)} className="mt-2 text-[11px]" style={{ color: 'var(--ink-4)', cursor: 'pointer' }}>Cancel</button>
        </div>
      ) : (
        <>
          <p className="text-[12px] leading-relaxed mb-3" style={{ color: 'var(--ink-3)' }}>
            Disconnect removes the stored credentials. The daemon reloads automatically.
          </p>
          <button onClick={onDisconnect} disabled={disconnecting} className="text-[12px] px-3 py-1.5 rounded-md transition-opacity"
            style={{ color: '#e53e3e', border: '1px solid #e53e3e', opacity: disconnecting ? 0.5 : 1, cursor: disconnecting ? 'not-allowed' : 'pointer', background: 'transparent' }}>
            {disconnecting ? 'Disconnecting…' : `Disconnect ${tracker.name}`}
          </button>
        </>
      )}
    </div>
  )
}

// ── Flow picker ───────────────────────────────────────────────────────────────
function TrackerSetup({ tracker, onSuccess }: { tracker: Tracker; onSuccess?: () => void }) {
  // Providers that offer BOTH OAuth and a token get a mode toggle.
  const dual = !!tracker.oauth && !!tracker.token
  const [mode, setMode] = useState<'oauth' | 'token'>(tracker.oauth ? 'oauth' : 'token')

  if (tracker.azure) return <AzureDevOpsSetup tracker={tracker} onSuccess={onSuccess} />

  return (
    <div style={{ background: 'var(--surface-2)' }}>
      {dual && (
        <div className="px-4 pt-2 pb-1 flex gap-2">
          <ModeTab label={tracker.oauth!.label} active={mode === 'oauth'} onClick={() => setMode('oauth')} />
          <ModeTab label={tracker.token!.label} active={mode === 'token'} onClick={() => setMode('token')} />
        </div>
      )}
      {mode === 'oauth' && tracker.oauth
        ? <OAuthSetup tracker={tracker} onSuccess={onSuccess} />
        : <TokenSetup tracker={tracker} onSuccess={onSuccess} />}
    </div>
  )
}

function ModeTab({ label, active, onClick }: { label: string; active: boolean; onClick: () => void }) {
  return (
    <button onClick={onClick} className="text-[11px] px-3 py-1 rounded-md"
      style={{ background: active ? 'var(--accent)' : 'var(--tint)', color: active ? '#fff' : 'var(--ink-3)', cursor: 'pointer' }}>
      {label}
    </button>
  )
}

// ── Browser OAuth (start_oauth + poll) ───────────────────────────────────────
function OAuthSetup({ tracker, onSuccess }: { tracker: Tracker; onSuccess?: () => void }) {
  const [status, setStatus] = useState<'idle' | 'waiting' | 'done' | 'error'>('idle')
  const [error, setError] = useState<string | null>(null)
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null)
  // mountedRef lets the async startOAuth body detect an unmount that happened
  // while awaiting mutate — before the interval is created and pollRef assigned.
  const mountedRef = useRef(true)

  useEffect(() => () => {
    mountedRef.current = false
    if (pollRef.current != null) clearInterval(pollRef.current)
  }, [])

  const startOAuth = async () => {
    setStatus('waiting'); setError(null)
    try {
      await mutate(`/api/auth/oauth/start?provider=${tracker.id}`, 'start_oauth', { provider: tracker.id })
      if (!mountedRef.current) return
      const deadline = Date.now() + OAUTH_DEADLINE_MS
      let stopped = false
      const id = setInterval(async () => {
        if (stopped) return
        if (Date.now() > deadline) {
          stopped = true; clearInterval(id); pollRef.current = null
          setStatus('error'); setError('Timed out — try again'); return
        }
        // A terminal error from the flow surfaces immediately (no 3-min wait).
        const oauthSt = await load<{ connected: boolean; error?: string | null }>(
          `/api/auth/oauth/status?provider=${tracker.id}`, 'get_oauth_status', { provider: tracker.id },
        ).catch(() => null)
        if (stopped) return
        if (oauthSt?.error) {
          stopped = true; clearInterval(id); pollRef.current = null
          setStatus('error'); setError(oauthSt.error); return
        }
        const data = await load<Record<string, unknown>>('/api/integrations', 'get_integrations').catch(() => null)
        if (stopped || !data) return
        if (data[tracker.id]) {
          stopped = true; clearInterval(id); pollRef.current = null
          setStatus('done'); clearProviderNotice(tracker.id); onSuccess?.()
        }
      }, 2_000)
      pollRef.current = id
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e)); setStatus('error')
    }
  }

  return (
    <div className="px-4 pb-4 pt-2" style={{ background: 'var(--surface-2)' }}>
      {status === 'idle' && (
        <div className="space-y-3">
          <p className="text-[12px] leading-relaxed" style={{ color: 'var(--ink-2)' }}>{tracker.oauth?.hint}</p>
          <button onClick={startOAuth} className="text-[12px] px-4 py-2 rounded-md font-medium"
            style={{ background: 'var(--accent)', color: '#fff', cursor: 'pointer' }}>
            Connect {tracker.name} →
          </button>
        </div>
      )}
      {status === 'waiting' && (
        <div className="space-y-2">
          <p className="text-[12px]" style={{ color: 'var(--ink-2)' }}>Your browser should have opened. Authorize the app, then come back here.</p>
          <p className="text-[11px]" style={{ color: 'var(--ink-4)' }}>Waiting for authorization…</p>
        </div>
      )}
      {status === 'done' && <p className="text-[12px]" style={{ color: 'var(--success)' }}>✓ Connected! Your tasks will appear shortly.</p>}
      {status === 'error' && (
        <div className="space-y-2">
          <p className="text-[12px]" style={{ color: '#e53e3e' }}>{error ?? 'OAuth failed.'}</p>
          <button onClick={() => setStatus('idle')} className="text-[11px]" style={{ color: 'var(--accent)', cursor: 'pointer' }}>Try again</button>
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

  const canSave = method.fields.filter((f) => f.required).every((f) => (values[f.name] ?? '').trim().length > 0)

  const save = async () => {
    if (!canSave || saving) return
    setSaving(true); setError(null)
    try {
      // save_integration_token (Rust) writes .env + reloads the daemon.
      await mutate('/api/auth/token', 'save_integration_token', { provider: tracker.id, fields: values })
      setDone(true); clearProviderNotice(tracker.id); onSuccess?.()
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Could not save credentials')
    } finally {
      setSaving(false)
    }
  }

  if (done) {
    return (
      <div className="px-4 pb-4 pt-2" style={{ background: 'var(--surface-2)' }}>
        <p className="text-[12px]" style={{ color: 'var(--success)' }}>✓ Connected! Your tasks will appear shortly.</p>
      </div>
    )
  }

  return (
    <div className="px-4 pb-4 pt-2 space-y-3" style={{ background: 'var(--surface-2)' }}>
      <p className="text-[12px] leading-relaxed" style={{ color: 'var(--ink-2)' }}>
        {method.hint}{' '}
        {method.url && (
          <a href={method.url} target="_blank" rel="noopener noreferrer" style={{ color: 'var(--accent)' }}>Open ↗</a>
        )}
      </p>
      {method.fields.map((f) => (
        <Field key={f.name} field={f} value={values[f.name] ?? ''}
          onChange={(v) => setValues((s) => ({ ...s, [f.name]: v }))}
          onEnter={save} />
      ))}
      {error && <p className="text-[11px]" style={{ color: '#e53e3e' }}>{error}</p>}
      {method.note && <p className="text-[11px] leading-relaxed" style={{ color: 'var(--ink-4)' }}>{method.note}</p>}
      <button onClick={save} disabled={!canSave || saving} className="text-[12px] px-4 py-2 rounded-md font-medium transition-opacity"
        style={{ background: 'var(--accent)', color: '#fff', opacity: !canSave || saving ? 0.5 : 1, cursor: !canSave || saving ? 'not-allowed' : 'pointer' }}>
        {saving ? 'Connecting…' : `Connect ${tracker.name}`}
      </button>
    </div>
  )
}

function Field({ field, value, onChange, onEnter }: {
  field: TokenField; value: string; onChange: (v: string) => void; onEnter?: () => void
}) {
  return (
    <label className="block">
      <span className="text-[11px]" style={{ color: 'var(--ink-3)' }}>{field.label}</span>
      <input
        type={field.password ? 'password' : 'text'}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={(e) => { if (e.key === 'Enter' && onEnter) onEnter() }}
        placeholder={field.placeholder}
        className="mt-1 w-full font-mono text-[11px] px-2 py-1.5 rounded-md border"
        style={{ color: 'var(--ink)', background: 'var(--surface)', borderColor: 'var(--rule)', outline: 'none' }}
      />
      {field.hint && <span className="text-[10px] leading-relaxed block mt-1" style={{ color: 'var(--ink-4)' }}>{field.hint}</span>}
    </label>
  )
}

// ── Azure DevOps (PAT → org → project → save) ────────────────────────────────
function AzureDevOpsSetup({ tracker, onSuccess }: { tracker: Tracker; onSuccess?: () => void }) {
  const [pat, setPat] = useState('')
  const [orgs, setOrgs] = useState<string[] | null>(null)
  const [selectedOrg, setSelectedOrg] = useState('')
  const [projects, setProjects] = useState<string[] | null>(null)
  const [selectedProject, setSelectedProject] = useState('')
  const [loading, setLoading] = useState<'orgs' | 'projects' | 'saving' | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [done, setDone] = useState(false)

  const lookupOrgs = async () => {
    if (!pat.trim()) return
    setLoading('orgs'); setError(null); setOrgs(null); setSelectedOrg(''); setProjects(null); setSelectedProject('')
    try {
      const json = await mutate<{ orgs?: string[] }>('/api/integrations/azure-devops/discover', 'discover_azure_devops', { pat: pat.trim() })
      setOrgs(json.orgs ?? [])
      if ((json.orgs ?? []).length === 1) { setSelectedOrg(json.orgs![0]); lookupProjects(json.orgs![0]) }
    } catch (e) { setError(typeof e === 'string' ? e : e instanceof Error ? e.message : 'Failed to fetch organisations') }
    finally { setLoading(null) }
  }

  const lookupProjects = async (org: string) => {
    if (!org) return
    setLoading('projects'); setError(null); setProjects(null); setSelectedProject('')
    try {
      const json = await mutate<{ projects?: string[] }>('/api/integrations/azure-devops/discover', 'discover_azure_devops', { pat: pat.trim(), org })
      setProjects(json.projects ?? [])
      if ((json.projects ?? []).length === 1) setSelectedProject(json.projects![0])
    } catch (e) { setError(typeof e === 'string' ? e : e instanceof Error ? e.message : 'Failed to fetch projects') }
    finally { setLoading(null) }
  }

  const handleOrgChange = (org: string) => { setSelectedOrg(org); if (org) lookupProjects(org) }

  const connect = async () => {
    if (!selectedOrg || !selectedProject) return
    setLoading('saving'); setError(null)
    try {
      await mutate('/api/auth/token', 'save_integration_token', {
        provider: 'azure_devops',
        fields: { url: `https://dev.azure.com/${selectedOrg}/${selectedProject}`, pat: pat.trim() },
      })
      setDone(true); clearProviderNotice('azure_devops'); onSuccess?.()
    } catch (e) { setError(typeof e === 'string' ? e : e instanceof Error ? e.message : 'Could not save credentials') }
    finally { setLoading(null) }
  }

  if (done) {
    return (
      <div className="px-4 pb-4 pt-2" style={{ background: 'var(--surface-2)' }}>
        <p className="text-[12px]" style={{ color: 'var(--success)' }}>✓ Connected! Your tasks will appear shortly.</p>
      </div>
    )
  }

  return (
    <div className="px-4 pb-4 pt-2 space-y-3" style={{ background: 'var(--surface-2)' }}>
      <p className="text-[12px] leading-relaxed" style={{ color: 'var(--ink-2)' }}>
        In Azure DevOps go to User settings → Personal access tokens → New token, scope{' '}
        <strong>Work Items → Read &amp; write</strong>.{' '}
        <a href="https://dev.azure.com" target="_blank" rel="noopener noreferrer" style={{ color: 'var(--accent)' }}>Open ↗</a>
      </p>
      <div className="flex gap-2">
        <input type="password" value={pat} onChange={(e) => setPat(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && lookupOrgs()} placeholder="Paste your PAT here"
          className="flex-1 font-mono text-[11px] px-2 py-1.5 rounded-md border"
          style={{ color: 'var(--ink)', background: 'var(--surface)', borderColor: 'var(--rule)', outline: 'none' }} />
        <button onClick={lookupOrgs} disabled={!pat.trim() || loading === 'orgs'}
          className="text-[11px] px-3 py-1.5 rounded-md shrink-0"
          style={{ background: 'var(--accent)', color: '#fff', opacity: (!pat.trim() || loading === 'orgs') ? 0.5 : 1, cursor: (!pat.trim() || loading === 'orgs') ? 'not-allowed' : 'pointer' }}>
          {loading === 'orgs' ? 'Looking up…' : 'Look up'}
        </button>
      </div>

      {orgs !== null && (
        orgs.length === 0
          ? <p className="text-[12px]" style={{ color: 'var(--ink-3)' }}>No organisations found for this PAT.</p>
          : (
            <label className="block">
              <span className="text-[11px]" style={{ color: 'var(--ink-3)' }}>Organisation</span>
              <select value={selectedOrg} onChange={(e) => handleOrgChange(e.target.value)}
                className="mt-1 w-full text-[12px] px-2 py-1.5 rounded-md border"
                style={{ color: 'var(--ink)', background: 'var(--surface)', borderColor: 'var(--rule)' }}>
                <option value="">— select org —</option>
                {orgs.map((o) => <option key={o} value={o}>{o}</option>)}
              </select>
            </label>
          )
      )}

      {projects !== null && selectedOrg && (
        loading === 'projects'
          ? <p className="text-[11px]" style={{ color: 'var(--ink-3)' }}>Loading projects…</p>
          : projects.length === 0
            ? <p className="text-[12px]" style={{ color: 'var(--ink-3)' }}>No projects found in this organisation.</p>
            : (
              <label className="block">
                <span className="text-[11px]" style={{ color: 'var(--ink-3)' }}>Project</span>
                <select value={selectedProject} onChange={(e) => setSelectedProject(e.target.value)}
                  className="mt-1 w-full text-[12px] px-2 py-1.5 rounded-md border"
                  style={{ color: 'var(--ink)', background: 'var(--surface)', borderColor: 'var(--rule)' }}>
                  <option value="">— select project —</option>
                  {projects.map((p) => <option key={p} value={p}>{p}</option>)}
                </select>
              </label>
            )
      )}

      {error && <p className="text-[11px]" style={{ color: '#e53e3e' }}>{error}</p>}

      {selectedOrg && selectedProject && (
        <button onClick={connect} disabled={loading === 'saving'} className="text-[12px] px-4 py-2 rounded-md font-medium transition-opacity"
          style={{ background: 'var(--accent)', color: '#fff', opacity: loading === 'saving' ? 0.5 : 1, cursor: loading === 'saving' ? 'not-allowed' : 'pointer' }}>
          {loading === 'saving' ? 'Connecting…' : 'Connect Azure DevOps'}
        </button>
      )}
    </div>
  )
}
