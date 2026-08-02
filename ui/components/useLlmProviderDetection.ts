//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The provider-detection hook — install probing, connectivity testing, vendor installs and
// in-app sign-ins for the coding-agent CLIs. Split out of <LlmProviderPicker> purely for
// size; it is the picker's state, and the picker re-exports it so existing imports from
// '@/components/LlmProviderPicker' keep resolving.
//
// # Who calls this
// <LlmProviderPicker>'s two owners — the setup wizard's Intelligence step and Settings →
// Intelligence — each call it once and thread the result through as props, so a single
// detection pass backs whichever screen is open.

import { useCallback, useEffect, useState } from 'react'
import { invoke } from '@/lib/bridge'
import { llmSignIn } from '@/lib/llm-providers'
import type { InstallOutcome, ProviderStatus, ProviderTestResult } from '@/lib/api-types'

/**
 * Probe which provider CLIs exist on this Mac (free, instant). Unlike before, this NO LONGER
 * fires a real connectivity test for every installed CLI on mount — that spent a request per
 * provider and surfaced scary failures for CLIs the user never even opened (e.g. a stray
 * cursor-agent that isn't signed in). Testing now happens in the DETAIL view, only for the
 * provider the user actually opened. `install` runs the vendor installer, then re-detects and
 * auto-tests just that one.
 */
/** How long "Checking what's installed…" stays on screen at minimum. See `detect`. */
const PROBE_MIN_VISIBLE_MS = 650

export function useLlmProviderDetection() {
  const [status, setStatus] = useState<Record<string, ProviderStatus>>({})
  const [scanning, setScanning] = useState(true)
  const [testingIds, setTestingIds] = useState<Set<string>>(new Set())
  const [installingIds, setInstallingIds] = useState<Set<string>>(new Set())
  const [signingIds, setSigningIds] = useState<Set<string>>(new Set())

  /** Free install-state probe. Re-run on demand (Rescan / after a manual install).
   *
   *  Paced. The probe answers in single-digit milliseconds, so the Rescan button's spinner
   *  used to appear and vanish inside one frame - the user pressed it, saw nothing change,
   *  and reasonably concluded it does nothing. The floor is on the STATE, not the work: the
   *  results land as soon as they arrive, this only keeps the evidence on screen long enough
   *  to be read. */
  const detect = useCallback(async () => {
    setScanning(true)
    const floor = new Promise<void>((r) => setTimeout(r, PROBE_MIN_VISIBLE_MS))
    try {
      const found = await invoke<ProviderStatus[]>('detect_llm_providers')
      setStatus(Object.fromEntries(found.map((p) => [p.id, p])))
    } catch {
      // A failed probe must not block the step: an un-probed provider renders as "can't
      // tell", and picking it is still allowed.
      setStatus({})
    } finally {
      await floor
      setScanning(false)
    }
  }, [])

  /** One real connectivity test — spends one request against the user's own subscription.
   *  `silent` runs + persists the test WITHOUT the "Testing…" spinner, so a confirm-in-the
   *  background (e.g. right after sign-in) doesn't yank the panel back to a spinner. */
  const testOne = useCallback(async (id: string, opts?: { silent?: boolean }) => {
    if (!opts?.silent) setTestingIds((prev) => new Set(prev).add(id))
    try {
      const result = await invoke<ProviderTestResult>('test_llm_provider', { id })
      setStatus((prev) => (prev[id] ? { ...prev, [id]: { ...prev[id], last_test: result } } : prev))
    } catch {
      // A failed probe CALL isn't evidence the provider stopped working — keep what was cached.
    } finally {
      if (!opts?.silent) {
        setTestingIds((prev) => {
          const next = new Set(prev)
          next.delete(id)
          return next
        })
      }
    }
  }, [])

  /** Run the vendor installer for one provider, then re-detect and (on success) auto-test it. */
  const install = useCallback(async (id: string): Promise<InstallOutcome> => {
    setInstallingIds((prev) => new Set(prev).add(id))
    try {
      const outcome = await invoke<InstallOutcome>('install_llm_provider', { id })
      await detect()
      if (outcome.ok) await testOne(id)
      return outcome
    } catch (e) {
      return { ok: false, message: String(e), path: null, command: '' }
    } finally {
      setInstallingIds((prev) => { const next = new Set(prev); next.delete(id); return next })
    }
  }, [detect, testOne])

  /** Run an interactive browser sign-in for a provider whose CLI authenticates against the
   *  user's own subscription - Cursor (`cursor_sign_in`), Codex (`codex_sign_in`), or Claude
   *  (`claude_sign_in`). Each tray command drives that vendor's `… login` and opens the browser;
   *  none takes a provider argument, so the id picks the command here. */
  const signIn = useCallback(async (id: string): Promise<InstallOutcome> => {
    // Resolved from `LLM_SIGN_IN`, the same record that gives LlmProviderDetail its button
    // copy - so a provider either has both a button and a command, or neither. Still no
    // default fallback: an unknown id must fail loudly rather than silently running some
    // other vendor's login (e.g. Cursor's) for it.
    const command = llmSignIn(id)?.trayCommand
    if (!command) {
      return { ok: false, message: `No in-app sign-in is wired up for "${id}".`, path: null, command: '' }
    }
    setSigningIds((prev) => new Set(prev).add(id))
    try {
      const outcome = await invoke<InstallOutcome>(command)
      if (outcome.ok) {
        // `cursor-agent login` exits 0 only AFTER the browser OAuth completes, so this is the
        // exact moment the user finished signing in. Flip the panel to "connected" NOW rather
        // than making them wait on a slow follow-up completion call, then confirm + persist it
        // with a silent background test (which corrects the badge if it somehow isn't usable).
        const result: ProviderTestResult = {
          id, outcome: { status: 'ok' }, elapsed_ms: 0, tested_at: new Date().toISOString(),
        }
        setStatus((prev) => {
          const cur = prev[id] ?? { id, installed: true, path: null, authenticated: null, last_test: null }
          return { ...prev, [id]: { ...cur, installed: true, last_test: result } }
        })
        void testOne(id, { silent: true })
      } else {
        // Sign-in didn't complete - refresh at least the install state.
        await detect()
      }
      return outcome
    } catch (e) {
      return { ok: false, message: String(e), path: null, command: '' }
    } finally {
      setSigningIds((prev) => {
        const next = new Set(prev)
        next.delete(id)
        return next
      })
    }
  }, [detect, testOne])

  useEffect(() => { detect() }, [detect])
  return { status, scanning, testingIds, installingIds, signingIds, testOne, install, signIn, rescan: detect }
}
