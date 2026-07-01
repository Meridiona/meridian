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
  TasksResponse, TaskSummary, TodayResponse,
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

  const loadWorklogs = useCallback((d: string) => {
    loadData<WorklogsResponse>(`/api/worklogs?day=${d}`, 'get_worklogs', { day: d })
      .then((res) => { setItems(res.items ?? []); setCounts(res.counts ?? {}); setLoading(false) })
      .catch(() => setLoading(false))
  }, [])

  const loadAux = useCallback(() => {
    loadData<IntegrationsResponse>('/api/integrations', 'get_integrations').then(setIntegrations).catch(() => {})
    loadData<TasksResponse>('/api/tasks', 'get_tasks').then((r) => setTasks(r.tasks ?? [])).catch(() => {})
    loadData<TodayResponse>('/api/today', 'get_today').then(setToday).catch(() => {})
  }, [])

  useEffect(() => {
    setLoading(true)
    loadWorklogs(day)
    loadAux()
    // Poll so approved → posted (daemon sweep) and fresh hours show up.
    const id = setInterval(() => { loadWorklogs(day); loadAux() }, 30_000)
    return () => clearInterval(id)
  }, [day, loadWorklogs, loadAux])

  const run = useCallback(async (key: string, fn: () => Promise<unknown>) => {
    setBusy(key)
    try {
      await fn()
    } catch (e) {
      alert(e instanceof Error ? e.message : 'Action failed')
    } finally {
      setBusy(null)
      loadWorklogs(day)
    }
  }, [day, loadWorklogs])

  const act = useCallback((id: number, action: 'approve' | 'unapprove') =>
    run(`wl:${id}`, () => mutate(`/api/worklogs/${id}`, 'worklog_action', { id, action })), [run])

  const reject = useCallback((id: number, correction: RejectCorrection) =>
    run(`wl:${id}`, () => mutate(`/api/worklogs/${id}`, 'worklog_action', { id, action: 'reject', ...correction })), [run])

  const saveEdit = useCallback((id: number, summary: string) =>
    run(`wl:${id}`, () => mutate(`/api/worklogs/${id}`, 'edit_worklog', { id, summary }, 'PATCH')), [run])

  const proposedAct = useCallback((id: number, action: 'approve' | 'dismiss') =>
    run(`prop:${id}`, () => mutate(`/api/proposed/${id}`, 'proposed_action', { id, action })), [run])
  const saveProposedTitle = useCallback((id: number, title: string) =>
    run(`prop:${id}`, () => mutate(`/api/proposed/${id}`, 'edit_proposed_title', { id, title }, 'PATCH')), [run])
  const saveProposedBody = useCallback((id: number, summary: string) =>
    run(`prop:${id}`, () => mutate(`/api/proposed/${id}`, 'edit_proposed_worklog', { id, summary }, 'PATCH')), [run])

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

  // Solo iff no tracker is connected; connectedProviderName is the single
  // connected provider's display name when EXACTLY one is on, else null.
  const { isSolo, connectedProviderName } = useMemo(() => {
    if (!integrations) return { isSolo: false, connectedProviderName: null as string | null }
    const on = PROVIDER_IDS.filter(id => integrations[id])
    return {
      isSolo: on.length === 0,
      connectedProviderName: on.length === 1 ? TRACKER_BY_ID[on[0]].name : null,
    }
  }, [integrations])

  const cleanupIssueCount = useMemo(() => cleanupCount(tasks, integrations), [tasks, integrations])

  const isToday = day === dayString(0)

  // Bundled write surface for the review overlay / detail cards.
  const actions: WorklogActions = {
    busy, act, reject, saveEdit, proposedAct, saveProposedTitle, saveProposedBody,
  }

  return {
    items, hourBuckets, counts, loading, busy, isToday, day, draftedIds,
    act, reject, saveEdit, proposedAct, saveProposedTitle, saveProposedBody, approveAll,
    actions, integrations, isSolo, connectedProviderName, tasks, cleanupIssueCount, today,
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
