//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The Board Cleanup swipe-card queue — ONE card, no second popup: every
// issue's fix control renders inline on CleanupCard itself, so fixing
// something is a single click. Self-contained backdrop (CleanupModal just
// mounts this, same convention as ReviewOverlay/ReviewModal). Fetches its own
// get_tasks + get_integrations (useTimelineData only exposes derived isSolo,
// not raw integrations). Snapshots the attention queue ONCE at open time
// (must-fix first, then optional hygiene, then stale/unclear) so it isn't
// reshuffled mid-review. A card's group (must/nice/review) is DERIVED live
// from its currently-visible issues, not fixed at snapshot — so once every
// issue on a must-fix ticket is resolved (fixed or ignored), it reclassifies
// as reviewable and the Keep button appears, without needing a separate
// "done" transition.

'use client'

import { useCallback, useEffect, useState } from 'react'
import { AnimatePresence, motion } from 'framer-motion'
import type { TaskSummary, TasksResponse, IntegrationsResponse } from '@/lib/api-types'
import { hasMustFix, type HygieneIssue } from '@/lib/hygiene'
import { load as loadData, mutate } from '@/lib/bridge'
import { filterByConnectedProviders } from '@/lib/integrations'
import { CleanupCard, type CleanupGroup } from './CleanupCard'

interface Stats { must: number; nice: number; review: number; ready: number; total: number }

function deriveGroup(issues: HygieneIssue[]): CleanupGroup {
  if (hasMustFix(issues)) return 'must'
  if (issues.length > 0) return 'nice'
  return 'review'
}

export function CleanupOverlay({ onClose }: { onClose: () => void }) {
  const [tasks, setTasks] = useState<TaskSummary[]>([])
  const [integrations, setIntegrations] = useState<IntegrationsResponse | null>(null)
  const [loading, setLoading] = useState(true)
  const [resolved, setResolved] = useState<Record<string, Set<string>>>({})
  const [index, setIndex] = useState(0)
  const [queue, setQueue] = useState<TaskSummary[] | null>(null)
  const [stats, setStats] = useState<Stats | null>(null)

  useEffect(() => {
    Promise.allSettled([
      loadData<TasksResponse>('/api/tasks', 'get_tasks').then(res => setTasks(res.tasks ?? [])),
      loadData<IntegrationsResponse>('/api/integrations', 'get_integrations').then(setIntegrations),
    ]).finally(() => setLoading(false))
  }, [])

  // Snapshot the queue + composition stats once the first load lands.
  useEffect(() => {
    if (loading || queue !== null) return
    const live = filterByConnectedProviders(tasks, integrations).filter(t => t.hygiene)
    const must: TaskSummary[] = []
    const nice: TaskSummary[] = []
    const review: TaskSummary[] = []
    for (const t of live) {
      const issues = t.hygiene!.issues ?? []
      if (hasMustFix(issues)) must.push(t)
      else if (issues.length > 0) nice.push(t)
      else if (t.hygiene!.bucket === 'looks_stale' || t.hygiene!.bucket === 'not_sure') review.push(t)
    }
    setQueue([...must, ...nice, ...review])
    setStats({ must: must.length, nice: nice.length, review: review.length, ready: live.length - (must.length + nice.length + review.length), total: live.length })
  }, [loading, queue, tasks, integrations])

  const visibleIssues = useCallback((t: TaskSummary): HygieneIssue[] => {
    const done = resolved[t.key]
    const issues = t.hygiene?.issues ?? []
    return done ? issues.filter(i => !done.has(i.code)) : issues
  }, [resolved])

  const markResolved = useCallback((taskKey: string, code: string) => {
    setResolved(prev => {
      const next = { ...prev }
      next[taskKey] = new Set(next[taskKey] ?? [])
      next[taskKey].add(code)
      return next
    })
  }, [])

  const unresolve = useCallback((taskKey: string, code: string) => {
    setResolved(prev => {
      const set = new Set(prev[taskKey] ?? [])
      set.delete(code)
      return { ...prev, [taskKey]: set }
    })
  }, [])

  // Ignore is a card-level quick dismissal (optional issues only — the
  // backend rejects must-fix codes). A real fix's control lives inline on the
  // card itself and calls markResolved directly from its own onApplied.
  const ignoreIssue = useCallback((taskKey: string, code: string) => {
    markResolved(taskKey, code)
    mutate('/api/triage/ignore', 'triage_ignore', { task_key: taskKey, code }).catch(() => unresolve(taskKey, code))
  }, [markResolved, unresolve])

  const keep = useCallback((taskKey: string) => {
    mutate('/api/triage/decision', 'triage_decision', { task_key: taskKey, decision: 'keep' }).catch(() => {})
    setIndex(i => i + 1)
  }, [])

  const current = queue && index < queue.length ? queue[index] : null
  const done = queue !== null && index >= queue.length
  const currentIssues = current ? visibleIssues(current) : []
  const currentGroup = deriveGroup(currentIssues)
  // Must-fix cards get no forward-nav escape either, matching CleanupCard's
  // own "no Keep escape until resolved" rule — otherwise Next/ArrowRight can
  // page straight past every must-fix ticket to a trailing review-only card,
  // click Keep once, and reach `done` (board reads "healthy") without ever
  // resolving the must-fix issues the composition bar is warning about.
  const canAdvance = currentGroup !== 'must'

  useEffect(() => {
    if (!done) return
    const id = setTimeout(onClose, 900)
    return () => clearTimeout(id)
  }, [done, onClose])

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === 'Escape') { onClose(); return }
      if (!queue) return
      if (e.key === 'ArrowRight' && canAdvance) setIndex(i => Math.min(queue.length - 1, i + 1))
      if (e.key === 'ArrowLeft') setIndex(i => Math.max(0, i - 1))
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [queue, onClose, canAdvance])

  const remaining = queue ? Math.max(0, queue.length - index) : 0

  return (
    <div className="absolute inset-0 z-50 flex items-center justify-center p-4 rise"
      style={{ background: 'rgba(20,16,40,0.5)', backdropFilter: 'blur(3px)' }} onClick={onClose}>
      <div className="w-full max-w-lg" onClick={e => e.stopPropagation()}>
        <div className="flex items-center justify-between mb-4 px-1">
          <p className="mt-label" style={{ color: '#fff' }}>
            Board clean-up {!loading && !done && `· ${remaining} left`}
          </p>
          <button onClick={onClose} aria-label="Close"
            className="inline-flex items-center justify-center rounded-full"
            style={{ width: 28, height: 28, color: '#fff', background: 'rgba(255,255,255,0.16)' }}>
            <span className="text-[16px] leading-none">×</span>
          </button>
        </div>

        {/* composition bar — red/orange/green = must-fix/nice-to-tidy/ready */}
        {stats && stats.total > 0 && (
          <div className="mb-3 px-1">
            <div className="flex h-2 rounded-full overflow-hidden" style={{ background: 'rgba(255,255,255,0.14)' }}>
              {stats.must > 0 && <span style={{ width: `${(stats.must / stats.total) * 100}%`, background: '#EF4444' }} />}
              {(stats.nice + stats.review) > 0 && <span style={{ width: `${((stats.nice + stats.review) / stats.total) * 100}%`, background: '#F97316' }} />}
              {stats.ready > 0 && <span style={{ width: `${(stats.ready / stats.total) * 100}%`, background: 'var(--color-state-approved)' }} />}
            </div>
            <p className="mt-body-sm mt-1.5" style={{ color: 'rgba(255,255,255,0.75)' }}>
              {stats.must + stats.nice + stats.review} of {stats.total} tickets need attention · {stats.must} must-fix
            </p>
          </div>
        )}

        {!loading && !done && queue && queue.length > 1 && (
          <div className="flex items-center justify-center gap-1.5 mb-4">
            {queue.map((q, i) => (
              <span key={q.key} className="rounded-full transition-all" style={{
                width: i === index ? 20 : 6, height: 6,
                background: '#fff', opacity: i === index ? 0.95 : i < index ? 0.4 : 0.25,
              }} />
            ))}
          </div>
        )}

        {loading ? (
          <div className="rounded-2xl p-10 text-center bg-card" style={{ border: '1px solid var(--t-card-border)' }}>
            <p className="mt-body-sm italic" style={{ color: 'var(--t-faint-2)' }}>Reading your board…</p>
          </div>
        ) : done ? (
          <div className="rounded-2xl p-10 text-center bg-card" style={{ border: '1px solid var(--t-card-border)' }}>
            <p className="mt-title" style={{ color: 'var(--color-state-approved)' }}>✓ Your board is healthy</p>
          </div>
        ) : (
          <AnimatePresence mode="popLayout">
            {current && (
              <motion.div key={current.key} initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: -8 }} transition={{ duration: 0.18 }}>
                <CleanupCard
                  task={current}
                  issues={currentIssues}
                  group={currentGroup}
                  onIgnore={code => ignoreIssue(current.key, code)}
                  onApplied={code => markResolved(current.key, code)}
                  onKeep={() => keep(current.key)}
                  onPrev={() => setIndex(i => Math.max(0, i - 1))}
                  onNext={() => { if (canAdvance) setIndex(i => Math.min((queue?.length ?? 1) - 1, i + 1)) }}
                  hasPrev={index > 0}
                  hasNext={!!queue && index < queue.length - 1 && canAdvance}
                />
              </motion.div>
            )}
          </AnimatePresence>
        )}

        {!loading && !done && (
          <p className="mt-body-sm mt-4 text-center" style={{ color: '#fff', opacity: 0.7 }}>
            ‹ › or arrow keys to browse · fix inline, Keep to move on
          </p>
        )}
      </div>
    </div>
  )
}
