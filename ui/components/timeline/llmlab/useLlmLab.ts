//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Data hook for the dev-only LLM Lab modal: the past-runs list, the selected
// run's detail, and the run action. While the selected experiment is `running`
// a 2s poll refreshes its detail (and the list row's progress count) - the
// execution happens in a detached `meridian llm-experiment exec` process, so
// polling the DB is the progress channel; there is no long-lived invoke to
// await. All three commands are dev-gated tray commands (commands/llm_lab.rs).

'use client'

import { useCallback, useEffect, useRef, useState } from 'react'
import { invoke, load } from '@/lib/bridge'
import type {
  LlmExperimentDetail,
  LlmExperimentSummary,
  RunLlmExperimentBody,
} from '@/lib/api-types'

const POLL_MS = 2_000

export function useLlmLab() {
  const [runs, setRuns] = useState<LlmExperimentSummary[]>([])
  const [detail, setDetail] = useState<LlmExperimentDetail | null>(null)
  const [starting, setStarting] = useState(false)
  const [error, setError] = useState<string | null>(null)
  // The id being polled - a ref so the interval callback never goes stale.
  const watchingId = useRef<number | null>(null)

  const refreshList = useCallback(async () => {
    try {
      setRuns(await load<LlmExperimentSummary[]>('/llm-lab', 'get_llm_experiments', { limit: 30 }))
    } catch {
      // A pre-061 DB or a closed pool reads as "no runs yet" - the composer
      // still works, so don't block the modal on the list.
    }
  }, [])

  const openRun = useCallback(async (id: number) => {
    setError(null)
    watchingId.current = id
    try {
      const d = await load<LlmExperimentDetail | null>('/llm-lab', 'get_llm_experiment', { id })
      if (watchingId.current === id) setDetail(d)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }, [])

  const closeRun = useCallback(() => {
    watchingId.current = null
    setDetail(null)
    setError(null)
  }, [])

  /** Start an experiment; resolves once the id exists (execution continues
   *  detached) and switches the view to that run. */
  const run = useCallback(async (body: RunLlmExperimentBody) => {
    setStarting(true)
    setError(null)
    try {
      const id = await invoke<number>('run_llm_experiment', { body })
      await openRun(id)
      refreshList()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setStarting(false)
    }
  }, [openRun, refreshList])

  useEffect(() => { refreshList() }, [refreshList])

  // Poll the open run while it is still executing.
  const running = detail?.status === 'running'
  const detailId = detail?.id ?? null
  useEffect(() => {
    if (!running || detailId === null) return
    const t = setInterval(async () => {
      try {
        const d = await load<LlmExperimentDetail | null>(
          '/llm-lab', 'get_llm_experiment', { id: detailId })
        if (watchingId.current === detailId && d) {
          setDetail(d)
          if (d.status !== 'running') refreshList()
        }
      } catch {
        // Transient read failure - keep polling; the run itself is unaffected.
      }
    }, POLL_MS)
    return () => clearInterval(t)
  }, [running, detailId, refreshList])

  return { runs, detail, starting, error, run, openRun, closeRun, refreshList }
}
