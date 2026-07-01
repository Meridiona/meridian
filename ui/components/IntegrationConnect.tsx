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
                  onDisconnect={() => handleDisconnect(t.id)} onChanged={onChanged}
                  githubProjectsSelected={integrations?.github_projects_selected} />
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
  tracker, syncError, disconnecting, onDisconnect, onChanged, githubProjectsSelected,
}: {
  tracker: Tracker; syncError?: string; disconnecting: boolean; onDisconnect: () => void; onChanged?: () => void
  /** Only meaningful for tracker.id === 'github' — undefined for every other tracker. */
  githubProjectsSelected?: boolean
}) {
  const [reauthorizing, setReauthorizing] = useState(false)
  const [pickingProjects, setPickingProjects] = useState(false)
  const cleanError = syncError ? syncError.replace(/^permission_error: |^sync_error: /, '') : null
  // GitHub's token alone doesn't sync anything — a Projects v2 board must be
  // selected too (discover_github_projects → save_integration_token). This is
  // exactly the gap a token connected outside the OAuth-connect picker (or an
  // account connected before this picker existed) is stuck in.
  const needsGithubProjects = tracker.id === 'github' && githubProjectsSelected === false

  return (
    <div className="px-4 pb-4 pt-2" style={{ background: 'var(--surface-2)' }}>
      {cleanError && !reauthorizing && !pickingProjects && (
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
      {needsGithubProjects && !reauthorizing && !pickingProjects && (
        <div className="mb-3 rounded-md px-3 py-2" style={{ background: '#eff6ff', border: '1px solid #bfdbfe' }}>
          <p className="text-[12px] leading-relaxed" style={{ color: '#1e40af' }}>
            No GitHub Projects selected — tasks won&apos;t sync yet.
          </p>
          <button onClick={() => setPickingProjects(true)} className="mt-2 text-[11px] px-3 py-1 rounded-md"
            style={{ background: '#1e40af', color: '#fff', cursor: 'pointer' }}>
            Select Projects
          </button>
        </div>
      )}
      {pickingProjects ? (
        <div className="mb-1">
          <GitHubProjectPicker onSuccess={() => { setPickingProjects(false); onChanged?.() }} />
          <button onClick={() => setPickingProjects(false)} className="mt-2 text-[11px]" style={{ color: 'var(--ink-4)', cursor: 'pointer' }}>Cancel</button>
        </div>
      ) : reauthorizing ? (
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

// Sentinel matched against the Rust error message when TRELLO_APP_KEY is unset.
// Extracted here so a rewording of the Rust error string is a single-site change.
const TRELLO_MISSING_KEY_SENTINEL = 'Power-Up app key'

// ── Browser OAuth (start_oauth + poll) ───────────────────────────────────────
function OAuthSetup({ tracker, onSuccess }: { tracker: Tracker; onSuccess?: () => void }) {
  const [status, setStatus] = useState<'idle' | 'waiting' | 'done' | 'error'>('idle')
  const [error, setError] = useState<string | null>(null)
  // Trello-specific: set when start_oauth rejects with the "Power-Up app key"
  // error so the user can supply their own key from https://trello.com/app-key.
  // Once set, the key is saved to .env before the next start_oauth call.
  const [apiKeyPrompt, setApiKeyPrompt] = useState(false)
  const [apiKey, setApiKey] = useState('')
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null)
  // mountedRef lets the async startOAuth body detect an unmount that happened
  // while awaiting mutate — before the interval is created and pollRef assigned.
  const mountedRef = useRef(true)

  // Set the flag TRUE on (re)mount, not just FALSE on unmount. Under React
  // StrictMode (on in `next dev`, which `tauri dev` serves) effects run
  // mount → cleanup → mount: the first cleanup flips this to false, and without
  // re-arming it here the component stays alive with mountedRef.current === false.
  // startOAuth's `if (!mountedRef.current) return` guard would then bail right
  // after `await mutate('start_oauth')`, so the poll interval that detects
  // success AND surfaces OAuth errors never starts — the UI hangs on "Waiting…"
  // forever even though the backend already wrote the credentials.
  useEffect(() => {
    mountedRef.current = true
    return () => {
      mountedRef.current = false
      if (pollRef.current != null) clearInterval(pollRef.current)
    }
  }, [])

  const startOAuth = async () => {
    setStatus('waiting'); setError(null)
    try {
      // For Trello: if a user-supplied API key is present, persist it to .env
      // before start_oauth reads it. This unblocks dev builds where the baked-in
      // DEFAULT_APP_KEY is empty. start_oauth_in_process re-parses .env on each
      // call, so the write-then-call ordering is sufficient.
      if (tracker.id === 'trello' && apiKey.trim()) {
        await mutate('/api/auth/token', 'save_integration_token', {
          provider: 'trello', fields: { api_key: apiKey.trim() },
        })
      }
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
          setStatus('done'); clearProviderNotice(tracker.id)
          // GitHub needs a Projects v2 board picked before sync does anything —
          // the picker (rendered below for status==='done') calls onSuccess
          // itself once a project is actually saved. Every other provider is
          // done syncing-wise the moment the token/OAuth store exists.
          if (tracker.id !== 'github') onSuccess?.()
        }
      }, 2_000)
      pollRef.current = id
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e)
      // Trello without a baked-in or user-supplied app key: show the API key
      // input so the user can unblock themselves without editing .env manually.
      if (tracker.id === 'trello' && msg.includes(TRELLO_MISSING_KEY_SENTINEL)) {
        setApiKeyPrompt(true); setStatus('idle')
      } else {
        setError(msg); setStatus('error')
      }
    }
  }

  return (
    <div className="px-4 pb-4 pt-2" style={{ background: 'var(--surface-2)' }}>
      {status === 'idle' && (
        <div className="space-y-3">
          <p className="text-[12px] leading-relaxed" style={{ color: 'var(--ink-2)' }}>{tracker.oauth?.hint}</p>
          {apiKeyPrompt && (
            <>
              <p className="text-[12px]" style={{ color: '#d97706' }}>
                A Trello API key is required.{' '}
                <a href="https://trello.com/app-key" target="_blank" rel="noopener noreferrer" style={{ color: 'var(--accent)' }}>Get it at trello.com/app-key ↗</a>
              </p>
              <Field
                field={{ name: 'api_key', label: 'API Key', placeholder: 'Paste your Trello API key', required: true }}
                value={apiKey}
                onChange={setApiKey}
                onEnter={() => { if (apiKey.trim()) void startOAuth() }}
                autoFocus
              />
            </>
          )}
          <button
            onClick={() => void startOAuth()}
            disabled={apiKeyPrompt && !apiKey.trim()}
            className="text-[12px] px-4 py-2 rounded-md font-medium transition-opacity"
            style={{
              background: 'var(--accent)', color: '#fff',
              opacity: apiKeyPrompt && !apiKey.trim() ? 0.5 : 1,
              cursor: apiKeyPrompt && !apiKey.trim() ? 'not-allowed' : 'pointer',
            }}>
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
      {status === 'done' && (
        tracker.id === 'github'
          ? <GitHubProjectPicker onSuccess={onSuccess} />
          : <p className="text-[12px]" style={{ color: 'var(--success)' }}>✓ Connected! Your tasks will appear shortly.</p>
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
      setError(typeof e === 'string' ? e : e instanceof Error ? e.message : 'Could not save credentials')
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

function Field({ field, value, onChange, onEnter, autoFocus }: {
  field: TokenField; value: string; onChange: (v: string) => void; onEnter?: () => void; autoFocus?: boolean
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
        autoFocus={autoFocus}
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
  const [showManualOrg, setShowManualOrg] = useState(false)
  const [manualOrg, setManualOrg] = useState('')

  const lookupOrgs = async () => {
    if (!pat.trim()) return
    setLoading('orgs'); setError(null); setOrgs(null); setSelectedOrg(''); setProjects(null); setSelectedProject(''); setShowManualOrg(false); setManualOrg('')
    try {
      const json = await mutate<{ orgs?: string[] }>('/api/integrations/azure-devops/discover', 'discover_azure_devops', { pat: pat.trim() })
      setOrgs(json.orgs ?? [])
      if ((json.orgs ?? []).length === 1) { setSelectedOrg(json.orgs![0]); lookupProjects(json.orgs![0]) }
    } catch (e) {
      const message = typeof e === 'string' ? e : e instanceof Error ? e.message : 'Failed to fetch organisations'
      setError(message)
      setShowManualOrg(message.includes('enter your org name manually below'))
    }
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

  const submitManualOrg = () => {
    const org = manualOrg.trim()
    if (org) { setSelectedOrg(org); lookupProjects(org) }
  }

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
        In Azure DevOps go to User settings → Personal access tokens → New token, set scope to{' '}
        <strong>All accessible organizations</strong> and enable <strong>Work Items → Read &amp; write</strong>.{' '}
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

      {showManualOrg && !orgs && (
        <div className="space-y-1.5">
          <label htmlFor="azure-devops-org" className="text-[11px]" style={{ color: 'var(--ink-3)' }}>Enter your org name manually:</label>
          <div className="flex gap-2">
            <input
              id="azure-devops-org"
              value={manualOrg}
              onChange={(e) => setManualOrg(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && submitManualOrg()}
              placeholder="e.g. my-company"
              className="flex-1 text-[11px] px-2 py-1.5 rounded-md border"
              style={{ color: 'var(--ink)', background: 'var(--surface)', borderColor: 'var(--rule)', outline: 'none' }} />
            <button
              onClick={submitManualOrg}
              disabled={!manualOrg.trim() || loading === 'projects'}
              className="text-[11px] px-3 py-1.5 rounded-md shrink-0"
              style={{ background: 'var(--accent)', color: '#fff', opacity: (!manualOrg.trim() || loading === 'projects') ? 0.5 : 1, cursor: (!manualOrg.trim() || loading === 'projects') ? 'not-allowed' : 'pointer' }}>
              {loading === 'projects' ? 'Looking up…' : 'Look up projects'}
            </button>
          </div>
        </div>
      )}

      {selectedOrg && selectedProject && (
        <button onClick={connect} disabled={loading === 'saving'} className="text-[12px] px-4 py-2 rounded-md font-medium transition-opacity"
          style={{ background: 'var(--accent)', color: '#fff', opacity: loading === 'saving' ? 0.5 : 1, cursor: loading === 'saving' ? 'not-allowed' : 'pointer' }}>
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
// identical either way.
type GithubProject = { id: string; title: string; owner: string }

function GitHubProjectPicker({ onSuccess }: { onSuccess?: () => void }) {
  const [projects, setProjects] = useState<GithubProject[] | null>(null)
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    load<{ projects?: GithubProject[] }>('/api/integrations/github/discover', 'discover_github_projects')
      .then((json) => { if (!cancelled) setProjects(json.projects ?? []) })
      .catch((e) => { if (!cancelled) setError(typeof e === 'string' ? e : e instanceof Error ? e.message : 'Failed to load GitHub Projects') })
      .finally(() => { if (!cancelled) setLoading(false) })
    return () => { cancelled = true }
  }, [])

  const toggle = (id: string) => {
    setSelected((prev) => {
      const next = new Set(prev)
      if (next.has(id)) next.delete(id); else next.add(id)
      return next
    })
  }

  const save = async () => {
    if (selected.size === 0 || saving) return
    setSaving(true); setError(null)
    try {
      await mutate('/api/auth/token', 'save_integration_token', {
        provider: 'github', fields: { project_ids: Array.from(selected).join(',') },
      })
      clearProviderNotice('github'); onSuccess?.()
    } catch (e) {
      setError(typeof e === 'string' ? e : e instanceof Error ? e.message : 'Could not save project selection')
    } finally {
      setSaving(false)
    }
  }

  if (loading) return <p className="text-[11px]" style={{ color: 'var(--ink-3)' }}>Loading your GitHub Projects…</p>

  if (error) return <p className="text-[12px]" style={{ color: '#e53e3e' }}>{error}</p>

  if (!projects || projects.length === 0) {
    return (
      <p className="text-[12px] leading-relaxed" style={{ color: 'var(--ink-3)' }}>
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
      <p className="text-[12px] leading-relaxed" style={{ color: 'var(--ink-2)' }}>
        Pick which GitHub Projects v2 boards to sync tasks from.
      </p>
      <div className="space-y-2 max-h-48 overflow-y-auto">
        {Object.entries(byOwner).map(([owner, ps]) => (
          <div key={owner}>
            <span className="text-[10px] uppercase tracking-wide" style={{ color: 'var(--ink-4)' }}>{owner}</span>
            {ps.map((p) => (
              <label key={p.id} className="flex items-center gap-2 py-1 text-[12px]" style={{ color: 'var(--ink)' }}>
                <input type="checkbox" checked={selected.has(p.id)} onChange={() => toggle(p.id)} />
                {p.title}
              </label>
            ))}
          </div>
        ))}
      </div>
      {error && <p className="text-[11px]" style={{ color: '#e53e3e' }}>{error}</p>}
      <button onClick={save} disabled={selected.size === 0 || saving} className="text-[12px] px-4 py-2 rounded-md font-medium transition-opacity"
        style={{ background: 'var(--accent)', color: '#fff', opacity: (selected.size === 0 || saving) ? 0.5 : 1, cursor: (selected.size === 0 || saving) ? 'not-allowed' : 'pointer' }}>
        {saving ? 'Saving…' : selected.size === 0 ? 'Select a project' : `Sync ${selected.size} project${selected.size === 1 ? '' : 's'}`}
      </button>
    </div>
  )
}
