//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The "Uninstall Meridian…" wizard — opened from the tray's right-click menu
// (tray.rs::open_uninstall_window), meant to be run BEFORE dragging
// Meridian.app to the Trash. Deleting the .app alone leaves the launchd agents
// (daemon, a11y-helper) running forever and does nothing to the Accessibility /
// Screen Recording / Input Monitoring grants in System Settings — macOS never
// revokes those just because the requesting app was deleted (see
// src/uninstall.rs's module doc). This wizard is the safe teardown: it always
// stops the agents + the in-process MLX server, offers three independent
// checkboxes for data/runtime/models, then points at System Settings for the
// permissions that survive on their own.
//
// Every read/write goes through `commands::uninstall` (get_uninstall_plan /
// execute_uninstall), which shells out to the staged `meridian uninstall` CLI —
// no logic is duplicated here.

import { useCallback, useEffect, useState } from 'react'
import type { CSSProperties, ReactNode } from 'react'
import { invoke, tauri } from '@/lib/bridge'
import type { UninstallItem, UninstallPlan, UninstallResult } from '@/lib/api-types'
import { Btn, Check, Kicker, Row, Spinner } from '../setup/atoms'

const SERIF: CSSProperties = { fontFamily: 'var(--font-instrument-serif), Georgia, serif' }

type Phase = 'loading' | 'plan-error' | 'select' | 'running' | 'done'

const EMPTY_PLAN: UninstallPlan = { agents: [], staged_binaries: [], data: [], runtime: [], models: [] }

export default function UninstallWizard() {
  const [phase, setPhase] = useState<Phase>('loading')
  const [plan, setPlan] = useState<UninstallPlan>(EMPTY_PLAN)
  const [result, setResult] = useState<UninstallResult | null>(null)
  const [removeData, setRemoveData] = useState(false)
  const [removeRuntime, setRemoveRuntime] = useState(false)
  const [removeModels, setRemoveModels] = useState(false)
  const [runErr, setRunErr] = useState('')

  useEffect(() => {
    invoke<UninstallPlan>('get_uninstall_plan')
      .then((p) => { setPlan(p); setPhase(p.error ? 'plan-error' : 'select') })
      .catch((e) => { setPlan({ ...EMPTY_PLAN, error: String(e) }); setPhase('plan-error') })
  }, [])

  const runUninstall = useCallback(async () => {
    setRunErr('')
    setPhase('running')
    try {
      const r = await invoke<UninstallResult>('execute_uninstall', {
        removeData, removeRuntime, removeModels,
      })
      setResult(r)
      setPhase('done')
    } catch (e) {
      setRunErr(String(e))
      setPhase('select')
    }
  }, [removeData, removeRuntime, removeModels])

  const openPane = useCallback((pane: string) => {
    invoke('open_permission_pane', { pane }).catch(() => {})
  }, [])

  const closeWindow = useCallback(() => { tauri()?.window.getCurrentWindow().close() }, [])
  const quit = useCallback(() => { invoke('quit_app').catch(() => {}) }, [])

  return (
    <div style={{ position: 'fixed', inset: 0, display: 'grid', placeItems: 'center', background: 'var(--t-panel)' }}>
      <div className="rise flex flex-col" style={{
        width: 660, height: 600, borderRadius: 18, background: 'var(--t-card)',
        border: '0.5px solid var(--t-card-border)', overflow: 'hidden', color: 'var(--t-title)',
        boxShadow: 'var(--pop-shadow)',
      }}>
        <div style={{ padding: '26px 32px 14px' }}>
          <Kicker style={{ marginBottom: 8 }}>{phase === 'done' ? 'Done' : 'Uninstall'}</Kicker>
          <h1 style={{ ...SERIF, fontSize: 26, lineHeight: 1.05, letterSpacing: '-.01em', color: 'var(--t-title)' }}>
            {phase === 'done' ? 'Cleaned up' : 'Uninstall Meridian'}
          </h1>
          {phase === 'select' && (
            <p style={{ fontSize: 12.5, lineHeight: 1.5, color: 'var(--t-muted)', marginTop: 8, maxWidth: 520 }}>
              This stops Meridian&apos;s background services first, so it&apos;s safe even if you never
              drag the app to the Trash. Choose what else to remove — this cannot be undone.
            </p>
          )}
        </div>

        <div className="nice-scroll" style={{ flex: 1, overflowY: 'auto', padding: '4px 32px 20px' }}>
          {phase === 'loading' && <Centered><Spinner size={22} /></Centered>}
          {phase === 'plan-error' && <PlanError message={plan.error ?? 'Unknown error'} />}
          {phase === 'select' && (
            <SelectStep
              plan={plan}
              removeData={removeData} setRemoveData={setRemoveData}
              removeRuntime={removeRuntime} setRemoveRuntime={setRemoveRuntime}
              removeModels={removeModels} setRemoveModels={setRemoveModels}
              err={runErr}
            />
          )}
          {phase === 'running' && (
            <Centered>
              <Spinner size={22} />
              <p style={{ fontSize: 12.5, color: 'var(--t-muted)', marginTop: 12 }}>Cleaning up…</p>
            </Centered>
          )}
          {phase === 'done' && result && <DoneStep result={result} openPane={openPane} />}
        </div>

        <div className="flex items-center justify-between" style={{ padding: '16px 28px', borderTop: '1px solid var(--t-hair)', background: 'var(--t-box)' }}>
          {phase === 'select' && (
            <>
              <Btn variant="ghost" onClick={closeWindow}>Cancel</Btn>
              <Btn variant="primary" onClick={runUninstall} style={{ background: 'var(--color-state-critical, #d24545)' }}>
                Uninstall
              </Btn>
            </>
          )}
          {phase === 'plan-error' && (
            <>
              <Btn variant="ghost" onClick={closeWindow}>Close</Btn>
              <Btn variant="secondary" onClick={() => setPhase('loading')}>Retry</Btn>
            </>
          )}
          {phase === 'running' && <span />}
          {phase === 'done' && (
            <>
              <Btn variant="secondary" onClick={closeWindow}>Close</Btn>
              <Btn variant="primary" onClick={quit}>Quit Meridian</Btn>
            </>
          )}
        </div>
      </div>
    </div>
  )
}

function Centered({ children }: { children: ReactNode }) {
  return <div style={{ display: 'grid', placeItems: 'center', height: '100%', paddingTop: 40 }}>{children}</div>
}

function PlanError({ message }: { message: string }) {
  return (
    <div style={{ padding: '20px 0' }}>
      <p style={{ fontSize: 13, color: 'var(--color-state-pending)' }}>
        Couldn&apos;t read the current install: {message}
      </p>
    </div>
  )
}

// ── Category checkbox row ─────────────────────────────────────────────────────
function OptionRow({ checked, onToggle, title, subtitle, items }: {
  checked: boolean; onToggle: () => void; title: string; subtitle: string; items: UninstallItem[]
}) {
  return (
    <Row tone={checked ? 'tint' : 'surface'} style={{ alignItems: 'flex-start', cursor: 'pointer' }}>
      <button
        onClick={onToggle}
        aria-pressed={checked}
        className="flex items-center justify-center shrink-0"
        style={{
          width: 20, height: 20, borderRadius: 6, marginTop: 1,
          background: checked ? 'var(--color-state-proposal)' : 'transparent',
          border: checked ? 'none' : '1px solid var(--t-card-border)',
        }}
      >
        {checked && <Check size={12} color="#fff" />}
      </button>
      <div onClick={onToggle} style={{ minWidth: 0, flex: 1 }}>
        <p style={{ fontSize: 13, fontWeight: 500, color: 'var(--t-title)' }}>{title}</p>
        <p style={{ fontSize: 11.5, color: 'var(--t-muted)', marginTop: 2 }}>{subtitle}</p>
        <p className="font-mono" style={{ fontSize: 10, color: 'var(--t-faint-2)', marginTop: 6 }}>
          {items.length === 0
            ? 'nothing found'
            : `${items.length} item${items.length === 1 ? '' : 's'} — ${items.slice(0, 3).map((i) => i.label).join(', ')}${items.length > 3 ? ', …' : ''}`}
        </p>
      </div>
    </Row>
  )
}

function SelectStep({
  plan, removeData, setRemoveData, removeRuntime, setRemoveRuntime, removeModels, setRemoveModels, err,
}: {
  plan: UninstallPlan
  removeData: boolean; setRemoveData: (v: boolean) => void
  removeRuntime: boolean; setRemoveRuntime: (v: boolean) => void
  removeModels: boolean; setRemoveModels: (v: boolean) => void
  err: string
}) {
  const alwaysRemoved = [...plan.agents, ...plan.staged_binaries]
  return (
    <div className="flex flex-col" style={{ gap: 10 }}>
      <OptionRow
        checked={removeData} onToggle={() => setRemoveData(!removeData)}
        title="Meridian data"
        subtitle="Activity database, tracker credentials, settings, logs, and telemetry"
        items={plan.data}
      />
      <OptionRow
        checked={removeRuntime} onToggle={() => setRemoveRuntime(!removeRuntime)}
        title="Python + MLX server"
        subtitle="The on-device model runtime Meridian downloaded to classify your activity"
        items={plan.runtime}
      />
      <OptionRow
        checked={removeModels} onToggle={() => setRemoveModels(!removeModels)}
        title="Downloaded AI models"
        subtitle="Meridian's own models in the shared HuggingFace cache — never touches models you downloaded for other tools"
        items={plan.models}
      />

      <div style={{ marginTop: 6, padding: '11px 13px', borderRadius: 11, background: 'var(--t-box)', border: '0.5px solid var(--t-card-border)' }}>
        <p style={{ fontSize: 11.5, color: 'var(--t-muted)' }}>
          Always stopped and removed: the background daemon and Accessibility helper
          {alwaysRemoved.length > 0 ? ` (${alwaysRemoved.length} item${alwaysRemoved.length === 1 ? '' : 's'})` : ''}.
        </p>
        <p style={{ fontSize: 11.5, color: 'var(--t-muted)', marginTop: 6 }}>
          Note: this does <b>not</b> revoke Meridian&apos;s Accessibility / Screen Recording / Input
          Monitoring grants — macOS keeps those in System Settings until you remove them yourself.
          The next screen has shortcuts for that.
        </p>
      </div>

      {err && <p style={{ fontSize: 11.5, color: 'var(--color-state-pending)' }}>{err}</p>}
    </div>
  )
}

function DoneStep({ result, openPane }: { result: UninstallResult; openPane: (pane: string) => void }) {
  return (
    <div className="flex flex-col" style={{ gap: 14 }}>
      <p style={{ fontSize: 12.5, color: 'var(--t-muted)' }}>
        Removed {result.removed.length} item{result.removed.length === 1 ? '' : 's'}.
        {result.errors.length > 0 && ` ${result.errors.length} couldn't be removed.`}
      </p>
      {result.errors.length > 0 && (
        <div className="font-mono" style={{ fontSize: 10.5, color: 'var(--color-state-pending)', background: 'var(--t-box)', borderRadius: 9, padding: '8px 11px' }}>
          {result.errors.map((e, i) => <div key={i}>{e}</div>)}
        </div>
      )}

      <div style={{ padding: '13px 15px', borderRadius: 13, border: '0.5px solid var(--t-card-border)', background: 'var(--t-box)' }}>
        <p style={{ fontSize: 13, fontWeight: 500, color: 'var(--t-title)' }}>Revoke permissions (optional)</p>
        <p style={{ fontSize: 11.5, color: 'var(--t-muted)', marginTop: 4, marginBottom: 10 }}>
          Deleting or uninstalling doesn&apos;t remove these grants from System Settings — remove them
          yourself if you don&apos;t want the entry lingering.
        </p>
        <div className="flex" style={{ gap: 8, flexWrap: 'wrap' }}>
          <Btn variant="secondary" size="sm" onClick={() => openPane('accessibility')}>Accessibility</Btn>
          <Btn variant="secondary" size="sm" onClick={() => openPane('screen_recording')}>Screen Recording</Btn>
          <Btn variant="secondary" size="sm" onClick={() => openPane('input_monitoring')}>Input Monitoring</Btn>
        </div>
      </div>

      <p style={{ fontSize: 11.5, color: 'var(--t-muted)' }}>
        You can now drag Meridian.app to the Trash — nothing will be left running.
      </p>
    </div>
  )
}
