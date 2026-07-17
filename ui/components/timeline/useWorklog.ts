//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The worklog state machine for one day-task, extracted from DayTaskDetailPanel
// so the panel stays presentational and this — the get/generate/approve flow and
// its every-provider error handling — lives in one testable place.
//
// Lifecycle: on mount (or when the selected task changes) it reads any existing
// draft; `generate` runs (or re-runs, overwriting) the centralised AI match/
// propose call; `approve` creates-if-proposed then posts the update and reflects
// the posted result locally. All three go through the Tauri bridge:
//   - get      → load  (flat args)   read, returns null when there's no draft
//   - generate → invoke (flat args)  the LLM call (can take ~a minute)
//   - approve  → mutate (body)       create + post, idempotent server-side

'use client'

import { useEffect, useState } from 'react'
import { load, invoke, mutate } from '@/lib/bridge'
import type { DayTaskWorklogDraft, ApproveWorklogResponse } from '@/lib/api-types'

/** Where the flow is right now — drives which footer control the panel shows. */
export type WorklogPhase = 'loading' | 'idle' | 'generating' | 'approving'

export interface WorklogState {
  draft: DayTaskWorklogDraft | null
  phase: WorklogPhase
  error: string | null
  /** True once the update has been posted — Generate/Approve are then disabled. */
  posted: boolean
  /** The user is confirming the post (Approve tapped, awaiting "Yes, post"). */
  confirming: boolean
  setConfirming: (v: boolean) => void
  /** Run (or regenerate, overwriting) the AI draft. */
  generate: () => void
  /** Approve the current draft: create-if-proposed, post the comment, link it. */
  approve: () => void
}

const errMsg = (e: unknown): string =>
  e instanceof Error ? e.message : typeof e === 'string' ? e : 'Something went wrong'

const API = '/api/day-task-worklog' // vestigial route label the bridge wants (Tauri-only now)

/** Own the worklog flow for `(day, taskId)`. Re-primes when either changes. */
export function useWorklog(day: string, taskId: string): WorklogState {
  const [draft, setDraft] = useState<DayTaskWorklogDraft | null>(null)
  const [phase, setPhase] = useState<WorklogPhase>('loading')
  const [error, setError] = useState<string | null>(null)
  const [confirming, setConfirming] = useState(false)

  // Load any existing draft when the selected task changes. No draft yet — a task
  // never generated, or a pre-060 DB — resolves to null, which is not an error.
  useEffect(() => {
    let alive = true
    setPhase('loading'); setError(null); setConfirming(false); setDraft(null)
    load<DayTaskWorklogDraft | null>(API, 'get_day_task_worklog', { day, task_id: taskId })
      .then(r => { if (alive) { setDraft(r); setPhase('idle') } })
      .catch(() => { if (alive) setPhase('idle') })
    return () => { alive = false }
  }, [day, taskId])

  const generate = () => {
    setPhase('generating'); setError(null); setConfirming(false)
    invoke<DayTaskWorklogDraft>('generate_day_task_worklog', { day, task_id: taskId })
      .then(r => { setDraft(r); setPhase('idle'); if (r.error) setError(r.error) })
      .catch(e => { setError(errMsg(e)); setPhase('idle') })
  }

  const approve = () => {
    setPhase('approving'); setError(null); setConfirming(false)
    mutate<ApproveWorklogResponse>(API, 'approve_day_task_worklog', { day, task_id: taskId })
      .then(r => {
        setPhase('idle')
        if (!r.posted || r.error) { setError(r.error || 'Could not post the update'); return }
        // Reflect the posted result locally so the panel updates without a reload.
        setDraft(d => d ? { ...d, state: 'posted', target_key: r.target_key, created_task_key: r.created_task_key, browse_url: r.browse_url } : d)
      })
      .catch(e => { setError(errMsg(e)); setPhase('idle') })
  }

  return {
    draft,
    phase,
    error,
    posted: draft?.state === 'posted',
    confirming,
    setConfirming,
    generate,
    approve,
  }
}
