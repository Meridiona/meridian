//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Module-level state for the three long-running "connect a tracker" flows in
// IntegrationConnect.tsx — browser/device OAuth, Azure DevOps (PAT → org →
// project), and the GitHub Projects v2 picker.
//
// WHY A STORE INSTEAD OF useState IN THE COMPONENTS: each flow is rendered
// inside an accordion row that unmounts the moment the user collapses it,
// switches provider, or closes Settings — and each holds genuinely long-running
// async state (GitHub's device-code flow polls for up to 15 min; Azure discovery
// is multi-step with a typed PAT; the picker holds a discovered board list and a
// checkbox selection). Component-local state would be destroyed on that unmount,
// losing the device code the user is mid-way through typing at github.com, the
// PAT + discovered orgs, or the ticked boards. Keying the state by provider in a
// store that outlives the panel means navigating away and back preserves it.
//
// As a bonus, moving the OAuth poll loop to module scope removes the StrictMode
// mount→cleanup→mount fragility the old in-component poll had to guard against
// with mountedRef re-arming — the interval simply isn't tied to a component
// lifecycle anymore.
//
// The components consume these via `useConnectStore(store, key)`; all I/O goes
// through the Tauri bridge exactly as before.

'use client'

import { useSyncExternalStore } from 'react'
import { load, invoke, mutate } from '@/lib/bridge'
import type { Tracker } from '@/lib/integrations'

const OAUTH_DEADLINE_MS = 180_000 // 3-minute browser-consent window (loopback: jira/trello)
const DEVICE_FLOW_DEADLINE_MS = 900_000 // 15 min — matches GitHub device-code expiry
// Sentinel matched against the Rust error message when TRELLO_APP_KEY is unset.
const TRELLO_MISSING_KEY_SENTINEL = 'Power-Up app key'

const errMsg = (e: unknown, fallback: string): string =>
  typeof e === 'string' ? e : e instanceof Error ? e.message : fallback

// Clear a provider's error notice immediately (don't wait for the next ETL poll).
// Fire-and-forget — a failure just means the banner clears on the next poll.
// Exported so TokenSetup (not store-backed) can share the one implementation.
export function clearProviderNotice(provider: string): void {
  void invoke('delete_notice', { noticeId: `pm.${provider}` }).catch(() => {})
}

// Abort a still-running jira/trello browser-OAuth attempt so a fresh start_oauth
// doesn't hit "already in progress". Only jira/trello are wired server-side
// (github's device flow has no loopback port to free). Fire-and-forget.
function cancelOAuthServer(provider: string): void {
  void mutate('/api/auth/oauth/cancel', 'cancel_oauth', { provider }).catch(() => {})
}

// ── Generic keyed external store ──────────────────────────────────────────────
// One store per flow; keyed by provider id so each provider's flow is isolated.
// `empty` is a single shared frozen default so useSyncExternalStore gets a STABLE
// snapshot reference for keys not yet written (it loops forever otherwise).

interface KeyedStore<T extends object> {
  empty: T
  get: (key: string) => T
  set: (key: string, next: Partial<T>) => void
  subscribe: (l: () => void) => () => void
}

function createKeyedStore<T extends object>(empty: T): KeyedStore<T> {
  const map = new Map<string, T>()
  const listeners = new Set<() => void>()
  const get = (key: string): T => map.get(key) ?? empty
  const set = (key: string, next: Partial<T>) => {
    map.set(key, { ...get(key), ...next })
    listeners.forEach((l) => l())
  }
  const subscribe = (l: () => void) => {
    listeners.add(l)
    return () => {
      listeners.delete(l)
    }
  }
  return { empty, get, set, subscribe }
}

/** Thin React view over a keyed store — re-renders the caller on any change. */
export function useConnectStore<T extends object>(store: KeyedStore<T>, key: string): T {
  return useSyncExternalStore(
    store.subscribe,
    () => store.get(key),
    () => store.empty,
  )
}

// ── OAuth flow ────────────────────────────────────────────────────────────────

export type OAuthStatus = 'idle' | 'waiting' | 'done' | 'error'

export interface OAuthState {
  status: OAuthStatus
  error: string | null
  /** GitHub device flow: the one-time code the user enters at verifyUri. */
  deviceCode: string | null
  verifyUri: string | null
  /** Trello: shown when start_oauth rejects for a missing Power-Up app key. */
  apiKeyPrompt: boolean
  apiKey: string
}

const OAUTH_EMPTY: OAuthState = Object.freeze({
  status: 'idle',
  error: null,
  deviceCode: null,
  verifyUri: null,
  apiKeyPrompt: false,
  apiKey: '',
})

export const oauthStore = createKeyedStore<OAuthState>(OAUTH_EMPTY)

// Active poll controllers, keyed by provider — module-level so the loop keeps
// running (and keeps updating oauthStore) across any panel unmount/remount.
const pollers = new Map<string, { stopped: boolean; id: ReturnType<typeof setInterval> }>()

function stopPoll(provider: string): void {
  const p = pollers.get(provider)
  if (p) {
    p.stopped = true
    clearInterval(p.id)
    pollers.delete(provider)
  }
}

export function setOAuthApiKey(provider: string, apiKey: string): void {
  oauthStore.set(provider, { apiKey })
}

/** Reset a finished (done/error/apiKeyPrompt) attempt back to a clean idle so a
 *  remounting panel starts fresh — but NEVER touch a 'waiting' attempt, which is
 *  the in-flight state we exist to preserve. Called by the panel on mount. */
export function resetOAuthIfSettled(provider: string): void {
  if (oauthStore.get(provider).status !== 'waiting') {
    oauthStore.set(provider, { ...OAUTH_EMPTY })
  }
}

/** Abort a 'waiting' or 'error' attempt back to idle: stop the poll, free the
 *  loopback port (jira/trello), reset local state. */
export function cancelOAuth(tracker: Tracker): void {
  stopPoll(tracker.id)
  if (tracker.id === 'jira' || tracker.id === 'trello') cancelOAuthServer(tracker.id)
  oauthStore.set(tracker.id, { status: 'idle', error: null, deviceCode: null, verifyUri: null })
}

/** Start (or restart) the browser/device OAuth flow for `tracker`. Idempotent
 *  against a double-tap: a second call while one is polling is ignored. */
export async function startOAuth(tracker: Tracker): Promise<void> {
  const provider = tracker.id
  if (oauthStore.get(provider).status === 'waiting') return
  const apiKey = oauthStore.get(provider).apiKey
  stopPoll(provider)
  oauthStore.set(provider, { status: 'waiting', error: null, deviceCode: null, verifyUri: null })
  try {
    // Trello: persist a user-supplied API key to .env before start_oauth reads
    // it (unblocks dev builds where the baked-in DEFAULT_APP_KEY is empty).
    if (provider === 'trello' && apiKey.trim()) {
      await mutate('/api/auth/token', 'save_integration_token', {
        provider: 'trello',
        fields: { api_key: apiKey.trim() },
      })
    }
    const res = await mutate<{ user_code?: string; verification_uri?: string }>(
      `/api/auth/oauth/start?provider=${provider}`,
      'start_oauth',
      { provider },
    )
    // GitHub device flow returns a one-time code + URL for the user to enter.
    if (res?.user_code) oauthStore.set(provider, { deviceCode: res.user_code })
    if (res?.verification_uri) oauthStore.set(provider, { verifyUri: res.verification_uri })
    // Device flow gives the user ~15 min; loopback (jira/trello) keeps 3 min.
    const deadline = Date.now() + (res?.user_code ? DEVICE_FLOW_DEADLINE_MS : OAUTH_DEADLINE_MS)
    const ctl = { stopped: false, id: undefined as unknown as ReturnType<typeof setInterval> }
    ctl.id = setInterval(async () => {
      if (ctl.stopped) return
      if (Date.now() > deadline) {
        stopPoll(provider)
        oauthStore.set(provider, { status: 'error', error: 'Timed out - try again' })
        return
      }
      // A terminal error from the flow surfaces immediately (no full-window wait).
      const oauthSt = await load<{ connected: boolean; error?: string | null }>(
        `/api/auth/oauth/status?provider=${provider}`,
        'get_oauth_status',
        { provider },
      ).catch(() => null)
      if (ctl.stopped) return
      if (oauthSt?.error) {
        stopPoll(provider)
        oauthStore.set(provider, { status: 'error', error: oauthSt.error })
        return
      }
      const data = await load<Record<string, unknown>>('/api/integrations', 'get_integrations').catch(() => null)
      if (ctl.stopped || !data) return
      if (data[provider]) {
        stopPoll(provider)
        oauthStore.set(provider, { status: 'done' })
        clearProviderNotice(provider)
      }
    }, 2_000)
    pollers.set(provider, ctl)
  } catch (e) {
    const msg = errMsg(e, 'OAuth failed')
    // Trello without an app key: prompt for one instead of erroring out.
    if (provider === 'trello' && msg.includes(TRELLO_MISSING_KEY_SENTINEL)) {
      oauthStore.set(provider, { status: 'idle', apiKeyPrompt: true })
    } else {
      oauthStore.set(provider, { status: 'error', error: msg })
    }
  }
}

// ── Azure DevOps flow ─────────────────────────────────────────────────────────

export interface AzureState {
  pat: string
  orgs: string[] | null
  selectedOrg: string
  projects: string[] | null
  selectedProject: string
  loading: 'orgs' | 'projects' | 'saving' | null
  error: string | null
  done: boolean
  reloadWarning: boolean
  showManualOrg: boolean
  manualOrg: string
}

const AZURE_EMPTY: AzureState = Object.freeze({
  pat: '',
  orgs: null,
  selectedOrg: '',
  projects: null,
  selectedProject: '',
  loading: null,
  error: null,
  done: false,
  reloadWarning: false,
  showManualOrg: false,
  manualOrg: '',
})

export const azureStore = createKeyedStore<AzureState>(AZURE_EMPTY)

const AZURE_KEY = 'azure_devops'
const azSet = (next: Partial<AzureState>) => azureStore.set(AZURE_KEY, next)
const azGet = () => azureStore.get(AZURE_KEY)

export const setAzurePat = (pat: string) => azSet({ pat })
export const setAzureSelectedProject = (selectedProject: string) => azSet({ selectedProject })
export const setAzureManualOrg = (manualOrg: string) => azSet({ manualOrg })

export async function azureLookupProjects(org: string): Promise<void> {
  if (!org) return
  azSet({ loading: 'projects', error: null, projects: null, selectedProject: '' })
  try {
    const json = await mutate<{ projects?: string[] }>('/api/integrations/azure-devops/discover', 'discover_azure_devops', { pat: azGet().pat.trim(), org })
    const projects = json.projects ?? []
    azSet({ projects, selectedProject: projects.length === 1 ? projects[0] : '' })
  } catch (e) {
    azSet({ error: errMsg(e, 'Failed to fetch projects') })
  } finally {
    azSet({ loading: null })
  }
}

export async function azureLookupOrgs(): Promise<void> {
  if (!azGet().pat.trim()) return
  azSet({ loading: 'orgs', error: null, orgs: null, selectedOrg: '', projects: null, selectedProject: '', showManualOrg: false, manualOrg: '' })
  try {
    const json = await mutate<{ orgs?: string[] }>('/api/integrations/azure-devops/discover', 'discover_azure_devops', { pat: azGet().pat.trim() })
    const orgs = json.orgs ?? []
    azSet({ orgs, loading: null })
    if (orgs.length === 1) {
      azSet({ selectedOrg: orgs[0] })
      await azureLookupProjects(orgs[0])
    }
  } catch (e) {
    const message = errMsg(e, 'Failed to fetch organisations')
    azSet({ error: message, loading: null, showManualOrg: message.includes('enter your org name manually below') })
  }
}

export function azureSelectOrg(org: string): void {
  azSet({ selectedOrg: org })
  if (org) void azureLookupProjects(org)
}

export function azureSubmitManualOrg(): void {
  const org = azGet().manualOrg.trim()
  if (org) azureSelectOrg(org)
}

export async function azureConnect(): Promise<void> {
  const { selectedOrg, selectedProject, pat } = azGet()
  if (!selectedOrg || !selectedProject) return
  azSet({ loading: 'saving', error: null })
  try {
    const res = await mutate<{ ok: boolean; reloaded: boolean }>('/api/auth/token', 'save_integration_token', {
      provider: AZURE_KEY,
      fields: { url: `https://dev.azure.com/${selectedOrg}/${selectedProject}`, pat: pat.trim() },
    })
    azSet({ done: true, reloadWarning: res?.reloaded === false, loading: null })
    clearProviderNotice(AZURE_KEY)
  } catch (e) {
    azSet({ error: errMsg(e, 'Could not save credentials'), loading: null })
  }
}

/** Reset Azure state to a clean slate — call on mount when the previous attempt
 *  finished, but preserve an in-flight typed PAT / discovery in progress. */
export function resetAzureIfSettled(): void {
  const s = azGet()
  if (s.done) azSet({ ...AZURE_EMPTY })
}

// ── GitHub Projects v2 picker ─────────────────────────────────────────────────

export type GithubProject = { id: string; title: string; owner: string }

export interface GithubPickerState {
  projects: GithubProject[] | null
  selected: Set<string>
  loading: boolean
  saving: boolean
  loaded: boolean
  loadError: string | null
  saveError: string | null
  reloadWarning: boolean
  saved: boolean
}

const GH_EMPTY: GithubPickerState = Object.freeze({
  projects: null,
  selected: new Set<string>(),
  loading: true,
  saving: false,
  loaded: false,
  loadError: null,
  saveError: null,
  reloadWarning: false,
  saved: false,
})

export const githubPickerStore = createKeyedStore<GithubPickerState>(GH_EMPTY)

const GH_KEY = 'github'
const ghSet = (next: Partial<GithubPickerState>) => githubPickerStore.set(GH_KEY, next)
const ghGet = () => githubPickerStore.get(GH_KEY)

/** Load the board list once. No-op if already loaded (successfully) or a save
 *  is in flight — but a PRIOR FAILURE must not block a retry. The `.catch()`
 *  below also sets `loaded: true` on error (so the picker isn't stuck showing
 *  a spinner forever), which without the `!s.loadError` check here made a
 *  single failed attempt (e.g. a transient race, or an attempt made before a
 *  token existed) latch permanently: `resetGithubPickerIfSaved()` only clears
 *  state once a save succeeds, and a save can never succeed while the picker
 *  has no projects to show — so every later, valid connect attempt (a fresh
 *  PAT, a fresh OAuth run) silently reused the same stale error forever. */
export function githubEnsureLoaded(): void {
  const s = ghGet()
  if ((s.loaded && !s.loadError) || s.saving) return
  ghSet({ loading: true, loadError: null })
  load<{ projects?: GithubProject[] }>('/api/integrations/github/discover', 'discover_github_projects')
    .then((json) => ghSet({ projects: json.projects ?? [], loading: false, loaded: true }))
    .catch((e) => ghSet({ loadError: errMsg(e, 'Failed to load GitHub Projects'), loading: false, loaded: true }))
}

export function githubToggle(id: string): void {
  const next = new Set(ghGet().selected)
  if (next.has(id)) next.delete(id)
  else next.add(id)
  ghSet({ selected: next })
}

export async function githubSave(): Promise<void> {
  const { selected, saving } = ghGet()
  if (selected.size === 0 || saving) return
  ghSet({ saving: true, saveError: null })
  try {
    const res = await mutate<{ ok: boolean; reloaded: boolean }>('/api/auth/token', 'save_integration_token', {
      provider: GH_KEY,
      fields: { project_ids: Array.from(selected).join(',') },
    })
    ghSet({ reloadWarning: res?.reloaded === false, saving: false, saved: true })
    clearProviderNotice(GH_KEY)
  } catch (e) {
    ghSet({ saveError: errMsg(e, 'Could not save project selection'), saving: false })
  }
}

/** Reset the picker so reopening after a completed save starts fresh (a fresh
 *  discover). Preserves an in-progress selection that hasn't been saved. */
export function resetGithubPickerIfSaved(): void {
  if (ghGet().saved) githubPickerStore.set(GH_KEY, { ...GH_EMPTY })
}
