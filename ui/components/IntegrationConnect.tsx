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

import { useEffect, useState } from 'react'
import { mutate, openExternal } from '@/lib/bridge'
import type { IntegrationsResponse } from '@/lib/api-types'
import { TRACKERS } from '@/lib/integrations'
import type { Tracker, TokenField } from '@/lib/integrations'
import { ProviderGlyph } from '@/components/atoms'
import {
  useConnectStore, clearProviderNotice,
  oauthStore, startOAuth, cancelOAuth, setOAuthApiKey, resetOAuthIfSettled,
  azureStore, azureLookupOrgs, azureSelectOrg, azureSubmitManualOrg, azureConnect,
  setAzurePat, setAzureSelectedProject, setAzureManualOrg, resetAzureIfSettled,
  githubPickerStore, githubEnsureLoaded, githubToggle, githubSave, resetGithubPickerIfSaved,
} from '@/components/integrationConnectStore'
import type { GithubProject } from '@/components/integrationConnectStore'

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
          return (
            <div key={t.id} style={{ borderTop: i > 0 ? '1px solid var(--t-hair)' : undefined }}>
              <button
                onClick={() => setOpen(isOpen ? null : t.id)}
                className="w-full flex items-center gap-3 px-4 py-3 text-left transition-colors"
                style={{ background: isOpen ? 'var(--t-box)' : 'var(--t-card)', cursor: 'pointer' }}
              >
                <ProviderGlyph provider={t.id} size={22} />
                <span className="flex flex-col min-w-0">
                  <span className="text-[13px]" style={{ color: 'var(--t-title)' }}>{t.name}</span>
                  {compact && !connected && <span className="text-[11px] truncate" style={{ color: 'var(--t-faint-2)' }}>{t.blurb}</span>}
                </span>
                {connected ? (
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
      {pickingProjects ? (
        <div className="mb-1">
          <GitHubProjectPicker onSuccess={() => { setPickingProjects(false); onChanged?.() }} />
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
          <p className="text-[12px] leading-relaxed mb-3" style={{ color: 'var(--t-faint)' }}>
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

// ── Flow picker ───────────────────────────────────────────────────────────────
function TrackerSetup({ tracker, onSuccess }: { tracker: Tracker; onSuccess?: () => void }) {
  // Providers that offer BOTH OAuth and a token get a mode toggle.
  const dual = !!tracker.oauth && !!tracker.token
  const [mode, setMode] = useState<'oauth' | 'token'>(tracker.oauth ? 'oauth' : 'token')

  if (tracker.azure) return <AzureDevOpsSetup tracker={tracker} onSuccess={onSuccess} />

  return (
    <div style={{ background: 'var(--t-box)' }}>
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
      style={{ background: active ? 'var(--color-state-proposal)' : 'color-mix(in srgb, var(--color-state-proposal) 10%, transparent)', color: active ? '#fff' : 'var(--t-faint)', cursor: 'pointer' }}>
      {label}
    </button>
  )
}

// ── Browser OAuth (start_oauth + poll) ───────────────────────────────────────
// A thin view over `oauthStore` — all flow state and the poll loop live in the
// store (integrationConnectStore.ts) so an in-flight attempt (crucially the
// device code the user is typing at github.com) survives this panel unmounting.
function OAuthSetup({ tracker, onSuccess }: { tracker: Tracker; onSuccess?: () => void }) {
  const { status, error, deviceCode, verifyUri, apiKeyPrompt, apiKey } = useConnectStore(oauthStore, tracker.id)

  // On mount, clear a settled (done/error) attempt back to idle — but never
  // touch a 'waiting' one, which is the in-flight state we exist to preserve.
  useEffect(() => { resetOAuthIfSettled(tracker.id) }, [tracker.id])

  // Fire onSuccess when the flow completes. GitHub defers this to its Projects
  // picker (rendered below for status==='done'), which calls onSuccess once a
  // board is actually saved. Every other provider is done the moment its store
  // exists. The store poll sets 'done' whether or not this panel is mounted; if
  // it isn't, reopening reloads integrations anyway, so nothing is missed.
  useEffect(() => {
    if (status === 'done' && tracker.id !== 'github') onSuccess?.()
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
                <a href="https://trello.com/app-key" onClick={(e) => { e.preventDefault(); openExternal('https://trello.com/app-key') }} style={{ color: 'var(--color-state-proposal)' }}>Get it at trello.com/app-key ↗</a>
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
              background: 'var(--color-state-proposal)', color: '#fff',
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
            <>
              <p className="text-[12px]" style={{ color: 'var(--t-muted)' }}>
                Enter this code at{' '}
                <a href={verifyUri ?? 'https://github.com/login/device'} target="_blank" rel="noopener noreferrer" style={{ color: 'var(--color-state-proposal)' }}>
                  {(verifyUri ?? 'https://github.com/login/device').replace(/^https?:\/\//, '')} ↗
                </a>{' '}
                (your browser should have opened it):
              </p>
              <div className="flex items-center gap-2">
                <code className="font-mono text-[16px] tracking-[0.2em] px-3 py-1.5 rounded-md border"
                  style={{ color: 'var(--t-title)', background: 'var(--t-card)', borderColor: 'var(--t-hair)' }}>
                  {deviceCode}
                </code>
                <button
                  onClick={() => { void navigator.clipboard?.writeText(deviceCode).catch(() => {}) }}
                  className="text-[11px] px-2 py-1 rounded-md"
                  style={{ color: 'var(--color-state-proposal)', border: '1px solid var(--t-hair)', cursor: 'pointer', background: 'transparent' }}>
                  Copy
                </button>
              </div>
              <p className="text-[11px]" style={{ color: 'var(--t-faint-2)' }}>Waiting for authorization…</p>
            </>
          ) : (
            <>
              <p className="text-[12px]" style={{ color: 'var(--t-muted)' }}>Your browser should have opened. Authorize the app, then come back here.</p>
              <p className="text-[11px]" style={{ color: 'var(--t-faint-2)' }}>Waiting for authorization…</p>
              {/* jira/trello only — a rejection Atlassian/Trello shows on THEIR
                  consent screen (e.g. "no Jira site access") never redirects
                  back here, so there's no automatic error to catch; without
                  this the user is stuck watching "Waiting…" for up to 5 min. */}
              {(tracker.id === 'jira' || tracker.id === 'trello') && (
                <button onClick={() => cancelOAuth(tracker)} className="text-[11px]" style={{ color: 'var(--t-faint)', cursor: 'pointer' }}>
                  Not going through? Cancel
                </button>
              )}
            </>
          )}
        </div>
      )}
      {status === 'done' && (
        tracker.id === 'github'
          ? <GitHubProjectPicker onSuccess={onSuccess} />
          : <p className="text-[12px]" style={{ color: 'var(--color-state-approved)' }}>✓ Connected! Your tasks will appear shortly.</p>
      )}
      {status === 'error' && (
        <div className="space-y-2">
          <p className="text-[12px]" style={{ color: 'var(--status-error-dot)' }}>{error ?? 'OAuth failed.'}</p>
          <button onClick={() => cancelOAuth(tracker)} className="text-[11px]" style={{ color: 'var(--color-state-proposal)', cursor: 'pointer' }}>
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
      clearProviderNotice(tracker.id); onSuccess?.()
    } catch (e) {
      setError(typeof e === 'string' ? e : e instanceof Error ? e.message : 'Could not save credentials')
    } finally {
      setSaving(false)
    }
  }

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
        {method.hint}{' '}
        {method.url && (
          <a href={method.url} onClick={(e) => { e.preventDefault(); openExternal(method.url!) }} style={{ color: 'var(--color-state-proposal)' }}>Open ↗</a>
        )}
      </p>
      {method.fields.map((f) => (
        <Field key={f.name} field={f} value={values[f.name] ?? ''}
          onChange={(v) => setValues((s) => ({ ...s, [f.name]: v }))}
          onEnter={save} />
      ))}
      {error && <p className="text-[11px]" style={{ color: 'var(--status-error-dot)' }}>{error}</p>}
      {method.note && <p className="text-[11px] leading-relaxed" style={{ color: 'var(--t-faint-2)' }}>{method.note}</p>}
      <button onClick={save} disabled={!canSave || saving} className="text-[12px] px-4 py-2 rounded-md font-medium transition-opacity"
        style={{ background: 'var(--color-state-proposal)', color: '#fff', opacity: !canSave || saving ? 0.5 : 1, cursor: !canSave || saving ? 'not-allowed' : 'pointer' }}>
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
        <a href="https://dev.azure.com" onClick={(e) => { e.preventDefault(); openExternal('https://dev.azure.com') }} style={{ color: 'var(--color-state-proposal)' }}>Open ↗</a>
      </p>
      <div className="flex gap-2">
        <input type="password" value={pat} onChange={(e) => setAzurePat(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && lookupOrgs()} placeholder="Paste your PAT here"
          className="flex-1 font-mono text-[11px] px-2 py-1.5 rounded-md border"
          style={{ color: 'var(--t-title)', background: 'var(--t-card)', borderColor: 'var(--t-hair)', outline: 'none' }} />
        <button onClick={lookupOrgs} disabled={!pat.trim() || loading === 'orgs'}
          className="text-[11px] px-3 py-1.5 rounded-md shrink-0"
          style={{ background: 'var(--color-state-proposal)', color: '#fff', opacity: (!pat.trim() || loading === 'orgs') ? 0.5 : 1, cursor: (!pat.trim() || loading === 'orgs') ? 'not-allowed' : 'pointer' }}>
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
              style={{ background: 'var(--color-state-proposal)', color: '#fff', opacity: (!manualOrg.trim() || loading === 'projects') ? 0.5 : 1, cursor: (!manualOrg.trim() || loading === 'projects') ? 'not-allowed' : 'pointer' }}>
              {loading === 'projects' ? 'Looking up…' : 'Look up projects'}
            </button>
          </div>
        </div>
      )}

      {selectedOrg && selectedProject && (
        <button onClick={connect} disabled={loading === 'saving'} className="text-[12px] px-4 py-2 rounded-md font-medium transition-opacity"
          style={{ background: 'var(--color-state-proposal)', color: '#fff', opacity: loading === 'saving' ? 0.5 : 1, cursor: loading === 'saving' ? 'not-allowed' : 'pointer' }}>
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

  if (loading) return <p className="text-[11px]" style={{ color: 'var(--ink-3)' }}>Loading your GitHub Projects…</p>

  if (loadError) return <p className="text-[12px]" style={{ color: 'var(--status-error-text)' }}>{loadError}</p>

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
                <input type="checkbox" checked={selected.has(p.id)} onChange={() => githubToggle(p.id)} />
                {p.title}
              </label>
            ))}
          </div>
        ))}
      </div>
      {saveError && <p className="text-[11px]" style={{ color: 'var(--status-error-text)' }}>{saveError}</p>}
      <button onClick={() => void githubSave()} disabled={selected.size === 0 || saving} className="text-[12px] px-4 py-2 rounded-md font-medium transition-opacity"
        style={{ background: 'var(--accent)', color: '#fff', opacity: (selected.size === 0 || saving) ? 0.5 : 1, cursor: (selected.size === 0 || saving) ? 'not-allowed' : 'pointer' }}>
        {saving ? 'Saving…' : selected.size === 0 ? 'Select a project' : `Sync ${selected.size} project${selected.size === 1 ? '' : 's'}`}
      </button>
      {reloadWarning && (
        <p className="text-[11px]" style={{ color: 'var(--t-faint)' }}>
          The daemon wasn&apos;t running — saved, will take effect on next start.
        </p>
      )}
    </div>
  )
}
