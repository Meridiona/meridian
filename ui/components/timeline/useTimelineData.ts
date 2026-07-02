//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Single top-of-app data hook for the one-pager Meridian Timeline. Generalized
// from the old useWorklogsForDay: it still owns a day's worklogs + every
// mutation, but now ALSO fetches integrations (→ solo/connected derivation),
// tasks (→ board-cleanup count, shared with the Cleanup/Tasks modals) and the
// day's presence/task rollup (get_today, for the Overview + Hour-detail
// time-by-app charts). Called ONCE in MeridianTimelineShell; every child reads
// from the same instance instead of re-fetching.

import { useCallback, useEffect, useMemo, useState } from 'react'
import type {
  WorklogItem, WorklogsResponse, IntegrationsResponse,
  TasksResponse, TaskSummary, TodayResponse, HourStatus, HourStatusResponse,
  HourReportEntry, HourReportsResponse,
} from '@/lib/api-types'
import { load as loadData, mutate } from '@/lib/bridge'
import { filterByConnectedProviders, TRACKER_BY_ID, type TrackerId } from '@/lib/integrations'
import { bucketByHour } from './timelineLayout'
import { dayString, itemKey, type RejectCorrection } from './types'

/** The mutation surface the review overlay / timeline cards act through — the
 *  subset of the hook that writes. Extracted so those components can take just
 *  what they need without depending on the whole hook's return type. */
export interface WorklogActions {
  busy: string | null
  act: (id: number, action: 'approve' | 'unapprove') => void
  reject: (id: number, correction: RejectCorrection) => void
  saveEdit: (id: number, summary: string) => void
  // Returns the outcome instead of alerting/throwing — a rematch can fail on
  // a legitimate business rule (the target ticket already has a worklog for
  // this exact day/hour — meridian_core::worklogs::RematchConflict, always a
  // hard block, never auto-merged) and the caller (ReviewCard) surfaces that
  // inline rather than a native alert() that's easy to miss in the Tauri
  // webview. `mergedIntoId` is currently always null; kept on the wire shape
  // so a future explicitly-confirmed merge doesn't need another contract change.
  rematch: (id: number, candidate: Candidate) => Promise<{ ok: boolean; error?: string; mergedIntoId?: number | null }>
  proposedAct: (id: number, action: 'approve' | 'dismiss') => void
  saveProposedTitle: (id: number, title: string) => void
  saveProposedBody: (id: number, summary: string) => void
}

const PROVIDER_IDS: TrackerId[] = ['jira', 'linear', 'github', 'trello', 'azure_devops']

/** Count of tasks that would show up in the board-cleanup modal: any connected
 *  task with a fixable hygiene issue or a stale/unsure bucket. Mirrors
 *  CleanupOverlay's queue-length so the Overview notice count matches the
 *  modal exactly. */
function cleanupCount(tasks: TaskSummary[], integrations: IntegrationsResponse | null): number {
  return filterByConnectedProviders(tasks, integrations)
    .filter(t => t.hygiene)
    .filter(t => {
      const issues = t.hygiene!.issues ?? []
      if (issues.length > 0) return true
      return t.hygiene!.bucket === 'looks_stale' || t.hygiene!.bucket === 'not_sure'
    }).length
}

export function useTimelineData(day: string) {
  const [items, setItems] = useState<WorklogItem[]>([])
  const [counts, setCounts] = useState<Record<string, number>>({})
  const [loading, setLoading] = useState(true)
  const [busy, setBusy] = useState<string | null>(null)
  const [integrations, setIntegrations] = useState<IntegrationsResponse | null>(null)
  const [tasks, setTasks] = useState<TaskSummary[]>([])
  const [today, setToday] = useState<TodayResponse | null>(null)
  const [hourStatus, setHourStatus] = useState<HourStatus[]>([])
  // Live "is tracking paused right now" — drives the blinking current-hour
  // badge (vs. the static historical badge from hourStatus[].paused).
  const [capturing, setCapturing] = useState<boolean | null>(null)
  // Per-hour activity-report markdown — the solo-mode timeline's per-row
  // content. Fetched unconditionally (cheap; today-only reader returns 24
  // empty entries for a past day) so it's ready the instant isSolo resolves.
  const [hourReports, setHourReports] = useState<HourReportEntry[]>([])

  const loadWorklogs = useCallback((d: string) => {
    loadData<WorklogsResponse>(`/api/worklogs?day=${d}`, 'get_worklogs', { day: d })
      .then((res) => { setItems(res.items ?? []); setCounts(res.counts ?? {}); setLoading(false) })
      .catch(() => setLoading(false))
  }, [])

  const loadHourStatus = useCallback((d: string) => {
    loadData<HourStatusResponse>(`/api/hour-status?day=${d}`, 'get_hour_status', { day: d })
      .then((res) => setHourStatus(res.hours ?? []))
      .catch(() => {})
  }, [])

  const loadHourReports = useCallback((d: string) => {
    loadData<HourReportsResponse>(`/api/hour-reports?day=${d}`, 'get_hour_reports', { day: d })
      .then((res) => setHourReports(res.hours ?? []))
      .catch(() => {})
  }, [])

  const loadAux = useCallback(() => {
    loadData<IntegrationsResponse>('/api/integrations', 'get_integrations').then(setIntegrations).catch(() => {})
    loadData<TasksResponse>('/api/tasks', 'get_tasks').then((r) => setTasks(r.tasks ?? [])).catch(() => {})
    loadData<TodayResponse>('/api/today', 'get_today').then(setToday).catch(() => {})
    loadData<{ running: boolean }>('/api/daemon/status', 'get_daemon_status')
      .then(s => setCapturing(s.running)).catch(() => {})
  }, [])

  useEffect(() => {
    setLoading(true)
    loadWorklogs(day)
    loadHourStatus(day)
    loadHourReports(day)
    loadAux()
    // Poll so approved → posted (daemon sweep), fresh hours, the
    // generating/paused badges, and today's activity reports all stay live.
    const id = setInterval(() => {
      loadWorklogs(day); loadHourStatus(day); loadHourReports(day); loadAux()
    }, 30_000)
    return () => clearInterval(id)
  }, [day, loadWorklogs, loadHourStatus, loadHourReports, loadAux])

  // Tauri's `invoke` rejects a failed command with whatever its `Err` variant
  // serialised to — here that's always a plain `String` (every tray command
  // returns `Result<T, String>`), NOT a JS `Error` object. So `e instanceof
  // Error` is always false for these rejections; the real server error text
  // lives in the string itself. Handle both shapes so a rematch conflict
  // ("PROJ-2 already has a worklog logged for this window…") actually
  // reaches the UI instead of being swallowed into a generic fallback.
  const errorMessage = (e: unknown): string =>
    e instanceof Error ? e.message : typeof e === 'string' ? e : 'Action failed'

  const run = useCallback(async (key: string, fn: () => Promise<unknown>) => {
    setBusy(key)
    try {
      await fn()
    } catch (e) {
      alert(errorMessage(e))
    } finally {
      // Reconciles with the server's actual row — rolls back an optimistic
      // patch (see `patchItem`) on failure, or confirms it on success.
      setBusy(null)
      loadWorklogs(day)
    }
  }, [day, loadWorklogs])

  // Same lifecycle as `run` (busy flag + reconciling refetch), but resolves
  // to the outcome (including the mutation's return value) instead of
  // alerting — for callers with a richer inline error surface than a native
  // alert() (see `rematch`/WorklogActions).
  const runQuiet = useCallback(async <T,>(key: string, fn: () => Promise<T>) => {
    setBusy(key)
    try {
      const value = await fn()
      return { ok: true as const, value }
    } catch (e) {
      return { ok: false as const, error: errorMessage(e) }
    } finally {
      setBusy(null)
      loadWorklogs(day)
    }
  }, [day, loadWorklogs])

  // Single source of truth for "patch a worklog/proposal's fields right now."
  // Both the timeline board and the review dialog render off the same `items`
  // array, so patching it here — instead of each caller keeping its own local
  // optimistic copy — is what makes an edit show up in both places instantly,
  // without waiting on the mutation's background refetch. The refetch inside
  // `run` still follows and reconciles with the server's actual row.
  const patchItem = useCallback((id: number, isProposed: boolean, patch: Partial<WorklogItem>) => {
    setItems(prev => prev.map(i => (i.id === id && i.is_proposed === isProposed) ? { ...i, ...patch } : i))
  }, [])

  const act = useCallback((id: number, action: 'approve' | 'unapprove') =>
    run(`wl:${id}`, () => mutate(`/api/worklogs/${id}`, 'worklog_action', { id, action })), [run])

  const reject = useCallback((id: number, correction: RejectCorrection) =>
    run(`wl:${id}`, () => mutate(`/api/worklogs/${id}`, 'worklog_action', { id, action: 'reject', ...correction })), [run])

  // Editing the summary is what actually gets posted to the tracker once
  // approved (post.rs's approved-sweep reads payload_json.summary fresh at
  // post time, so whatever's saved here is what lands in the PM comment).
  const saveEdit = useCallback((id: number, summary: string) => {
    patchItem(id, false, { summary, edited: true })
    return run(`wl:${id}`, () => mutate(`/api/worklogs/${id}`, 'edit_worklog', { id, summary }, 'PATCH'))
  }, [run, patchItem])

  // Re-match a real worklog to a different ticket — distinct from reject's
  // correctedTaskKey: this keeps the drafted worklog alive against the new
  // ticket instead of dismissing it. The write is logged server-side
  // (pm_worklog_feedback, feedback_kind='rematch') for traceability. Takes
  // the full candidate (not just the key) so the title patches immediately
  // too, instead of waiting on the refetch's pm_tasks join to resolve it.
  const rematch = useCallback(async (id: number, candidate: Candidate) => {
    patchItem(id, false, { task_key: candidate.key, task_title: candidate.title })
    const result = await runQuiet(`wl:${id}`, () =>
      mutate<{ mergedIntoId?: number | null }>(`/api/worklogs/${id}`, 'rematch_worklog', { id, taskKey: candidate.key }, 'PATCH'))
    if (!result.ok) return { ok: false as const, error: result.error }
    return { ok: true as const, mergedIntoId: result.value.mergedIntoId ?? null }
  }, [runQuiet, patchItem])

  const proposedAct = useCallback((id: number, action: 'approve' | 'dismiss') =>
    run(`prop:${id}`, () => mutate(`/api/proposed/${id}`, 'proposed_action', { id, action })), [run])
  const saveProposedTitle = useCallback((id: number, title: string) => {
    patchItem(id, true, { task_title: title })
    return run(`prop:${id}`, () => mutate(`/api/proposed/${id}`, 'edit_proposed_title', { id, title }, 'PATCH'))
  }, [run, patchItem])
  const saveProposedBody = useCallback((id: number, summary: string) => {
    patchItem(id, true, { summary })
    return run(`prop:${id}`, () => mutate(`/api/proposed/${id}`, 'edit_proposed_worklog', { id, summary }, 'PATCH'))
  }, [run, patchItem])

  const draftedIds = items.filter(i => i.state === 'drafted' && i.summary.trim()).map(i => i.id)
  const approveAll = useCallback(async () => {
    setBusy('all')
    const ids = items.filter(i => i.state === 'drafted' && i.summary.trim()).map(i => i.id)
    try {
      await Promise.all(ids.map(id =>
        mutate(`/api/worklogs/${id}`, 'worklog_action', { id, action: 'approve' })))
    } finally { setBusy(null); loadWorklogs(day) }
  }, [day, loadWorklogs, items])

  const hourBuckets = useMemo(() => bucketByHour(items), [items])

  // Solo iff no tracker is connected; connectedProviderName/Id are the single
  // connected provider's display name/id when EXACTLY one is on, else null.
  const { isSolo, connectedProviderName, connectedProviderId } = useMemo(() => {
    if (!integrations) return { isSolo: false, connectedProviderName: null as string | null, connectedProviderId: null as string | null }
    const on = PROVIDER_IDS.filter(id => integrations[id])
    return {
      isSolo: on.length === 0,
      connectedProviderName: on.length === 1 ? TRACKER_BY_ID[on[0]].name : null,
      connectedProviderId: on.length === 1 ? on[0] : null,
    }
  }, [integrations])

  const cleanupIssueCount = useMemo(() => cleanupCount(tasks, integrations), [tasks, integrations])

  const isToday = day === dayString(0)

  // Bundled write surface for the review overlay / detail cards.
  const actions: WorklogActions = {
    busy, act, reject, saveEdit, rematch, proposedAct, saveProposedTitle, saveProposedBody,
  }

  return {
    items, hourBuckets, counts, loading, busy, isToday, day, draftedIds,
    act, reject, saveEdit, rematch, proposedAct, saveProposedTitle, saveProposedBody, approveAll,
    actions, integrations, isSolo, connectedProviderName, connectedProviderId, tasks, cleanupIssueCount, today,
    hourStatus, capturing, hourReports,
  }
}

export type TimelineData = ReturnType<typeof useTimelineData>
export type Candidate = { key: string; title: string }

/** Shared candidate-task fetch for the reject/attribution picker. */
export async function fetchRejectCandidates(excludeKey: string): Promise<Candidate[]> {
  const data = await loadData<{ tasks: { key: string; title: string }[] }>('/api/tasks', 'get_tasks')
  return (data.tasks ?? [])
    .map((t) => ({ key: t.key, title: t.title }))
    .filter((c) => c.key !== excludeKey)
}

export { itemKey }
