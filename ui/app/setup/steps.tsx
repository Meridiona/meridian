//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// Wizard step bodies + Welcome + Completion + the STEPS meta (ported from the
// design's steps.jsx). Every body is driven by the live `Wiz` handle built in
// page.tsx — permissions, MLX status, hardware specs, and OAuth state are all
// real. Nothing here fabricates data: the spec panel and memory gauge render
// detected hardware, and the model tiles map to real checkpoints.

import type { CSSProperties, ReactNode } from 'react'
import { Btn, Bar, Check, Kicker, PermIcon, Row, Spinner } from './atoms'
import {
  APP, MODEL_RAM_GB, PERMISSIONS, fmtSize,
} from './data'
import type {
  DownloadProgress, MlxStatusResponse, SystemSpecs,
} from './data'
import type { IntegrationsResponse } from '@/lib/api-types'
import { TRACKERS } from '@/lib/integrations'
import ConnectTrackers from '@/components/IntegrationConnect'

const SERIF: CSSProperties = { fontFamily: 'var(--font-instrument-serif), Georgia, serif' }

/** The live wizard handle page.tsx builds and threads to every step body. */
export interface Wiz {
  // Step 1 — permissions (live, polled every 2 s)
  perms: { accessibility: boolean | null; screen: boolean | null; input: boolean | null }
  openPane: (pane: string) => void
  grantScreen: () => void
  grantInput: () => void
  // Step 2 — local intelligence (live MLX status + detected specs)
  specs: SystemSpecs | null
  mlx: MlxStatusResponse | null
  downloading: boolean    // runtime tarball in flight
  prefetching: boolean    // model-set download in flight
  modelReady: boolean     // every pipeline model is on disk
  progress: DownloadProgress | null
  speed: number | null    // live download speed in bytes/sec (null until known)
  err: string             // last provisioning error ('' when none) — drives Retry
  retryModel: () => void  // re-arm runtime/model provisioning after an error
  // Step 3 — integrations (live connected-state from get_integrations)
  integrations: IntegrationsResponse | null
  refetchIntegrations: () => void
}

// ── STEP 1 — Permissions ──────────────────────────────────────────────────────
function PermissionsBody({ wiz }: { wiz: Wiz }) {
  return (
    <div className="flex flex-col" style={{ gap: 9 }}>
      {PERMISSIONS.map((p) => {
        const granted = !!wiz.perms[p.id]
        return (
          <Row key={p.id} tone={granted ? 'tint' : 'surface'}>
            <span className="flex items-center justify-center shrink-0" style={{
              width: 34, height: 34, borderRadius: 10,
              background: granted ? 'color-mix(in srgb, var(--color-state-proposal) 12%, transparent)' : 'var(--t-box)',
              color: granted ? 'var(--color-state-proposal)' : 'var(--t-faint)',
              border: '0.5px solid var(--t-card-border)',
            }}><PermIcon icon={p.icon} /></span>

            <div style={{ flex: 1, minWidth: 0 }}>
              <div className="flex items-center" style={{ gap: 8 }}>
                <span style={{ fontSize: 13.5, fontWeight: 500, color: 'var(--t-title)' }}>{p.name}</span>
                <span className="font-mono" style={{ fontSize: 9, letterSpacing: '.1em', color: 'var(--t-faint)', border: '0.5px solid var(--t-card-border)', borderRadius: 4, padding: '1px 5px' }}>REQUIRED</span>
              </div>
              <p style={{ fontSize: 11.5, lineHeight: 1.4, color: 'var(--t-faint)', marginTop: 3 }}>{p.desc}</p>
            </div>

            <div className="shrink-0">
              {granted
                ? <span className="flex items-center" style={{ gap: 6, fontSize: 12, color: 'var(--color-state-approved)', fontWeight: 500 }}><Check size={15} color="var(--color-state-approved)" />Granted</span>
                : <Btn size="sm" variant="secondary" onClick={() => p.id === 'input' ? wiz.grantInput() : p.id === 'screen' ? wiz.grantScreen() : wiz.openPane(p.pane)}>Open Settings</Btn>}
            </div>
          </Row>
        )
      })}
      <p className="flex items-center" style={{ gap: 7, fontSize: 11, color: 'var(--t-faint)', marginTop: 3 }}>
        <span style={{ width: 5, height: 5, borderRadius: 99, background: 'var(--color-state-approved)' }} />
        Everything is processed on your Mac. Meridian has no servers to send to.
      </p>
    </div>
  )
}

// ── STEP 2 — Local intelligence ──────────────────────────────────────────────

/** One provisioning sub-step (engine install / model download). The leading
 *  glyph updates live as the phase advances: hollow ○ (pending) → spinner
 *  (active) → ✓ (done), so the two steps visibly tick off in sequence. */
type StepState = 'pending' | 'active' | 'done' | 'error'
function ProvisionStep({ state, label }: { state: StepState; label: string }) {
  return (
    <div className="flex items-center" style={{ gap: 10 }}>
      <span className="flex items-center justify-center shrink-0" style={{ width: 20, height: 20 }}>
        {state === 'done'
          ? <Check size={16} color="var(--color-state-approved)" w={2.2} />
          : state === 'error'
            ? <span style={{ width: 12, height: 12, borderRadius: 99, background: 'var(--color-state-pending)' }} />
            : state === 'active'
              ? <Spinner size={15} width={2} />
              : <span style={{ width: 12, height: 12, borderRadius: 99, border: '1.5px solid var(--t-card-border)' }} />}
      </span>
      <span style={{
        fontSize: 12.5,
        color: state === 'pending' ? 'var(--t-faint)' : state === 'error' ? 'var(--color-state-pending)' : 'var(--t-title)',
        fontWeight: state === 'done' ? 500 : 400,
      }}>{label}</span>
    </div>
  )
}

/** Self-checkable causes shown when provisioning hits a network/download error —
 *  ordered most-likely first, phrased so a non-technical user can fix it during
 *  setup (the download reaches GitHub over the network; corporate proxy/VPN/
 *  captive-portal are the usual blockers). */
const SETUP_NET_HINTS: { t: string; d: string }[] = [
  { t: 'Internet connection', d: 'Confirm you’re online. On café or hotel Wi-Fi, open a browser and finish any “sign in to connect” page, then Try again.' },
  { t: 'VPN or firewall', d: 'A work VPN or firewall can block the download. Pause the VPN or switch to another network.' },
  { t: 'Network proxy', d: 'If your Mac uses a proxy (System Settings → Network → Proxies), Meridian may not pick it up. Try a network without a proxy.' },
  { t: 'Security software', d: 'Tools that inspect secure connections can interrupt the download — allow Meridian or pause them briefly.' },
]

function MLXBody({ wiz }: { wiz: Wiz }) {
  const m = wiz.mlx
  const runtimeInstalled = !!(m && (m.runtime_found || m.runtime_installed))
  const unavailable = !!(m && !runtimeInstalled && !m.download_available)
  const pct = wiz.progress && wiz.progress.total > 0
    ? Math.min(100, Math.round((wiz.progress.received / wiz.progress.total) * 100)) : null
  const showErr = (!!wiz.err && !wiz.modelReady) || unavailable
  const working = !wiz.modelReady && !showErr
  const installStepState: StepState =
    runtimeInstalled ? 'done' : wiz.downloading ? 'active' : wiz.err ? 'error' : 'pending'
  const modelStepState: StepState =
    wiz.modelReady ? 'done' : wiz.prefetching ? 'active' : runtimeInstalled && !!wiz.err ? 'error' : 'pending'

  return (
    <div className="flex flex-col items-center justify-center" style={{ minHeight: 300, textAlign: 'center', padding: '8px 8px 4px' }}>
      {/* State glyph — one calm circle, mirroring the Completion mark */}
      <span className="flex items-center justify-center mer-pop" style={{
        width: 60, height: 60, borderRadius: 99, marginBottom: 24,
        background: showErr ? 'var(--t-box)' : 'color-mix(in srgb, var(--color-state-proposal) 12%, transparent)',
        color: showErr ? 'var(--color-state-pending)' : 'var(--color-state-proposal)',
        border: showErr ? '0.5px solid var(--t-card-border)' : 'none',
      }}>
        {wiz.modelReady
          ? <Check size={28} color="var(--color-state-proposal)" w={2.2} />
          : showErr
            ? <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round"><path d="M12 7.5v6" /><circle cx="12" cy="17" r="0.7" fill="currentColor" stroke="none" /></svg>
            : <Spinner size={24} width={2} />}
      </span>

      {/* Progress — only while working. The MB readout makes it tangibly alive:
          the numbers climb even when % barely moves on a slow shard. */}
      {working && (
        <div style={{ width: 300, minHeight: 30 }}>
          {pct !== null ? (
            <>
              <Bar pct={pct} />
              <p className="font-mono tnum" style={{ fontSize: 11, color: 'var(--t-faint)', marginTop: 9, letterSpacing: '.02em' }}>
                {Math.round((wiz.progress?.received ?? 0) / 1_048_576)} / {Math.round((wiz.progress?.total ?? 0) / 1_048_576)} MB · {pct}%
                {wiz.speed && wiz.speed > 0 ? ` · ${(wiz.speed / 1_048_576).toFixed(1)} MB/s` : ''}
              </p>
            </>
          ) : (
            <p className="font-mono" style={{ fontSize: 11.5, color: 'var(--t-faint)' }}>{wiz.progress?.message ?? 'Preparing…'}</p>
          )}
        </div>
      )}

      {/* Two-step checklist — each row ticks to ✓ as that phase completes, so
          "engine" and "models" read as distinct, sequential steps. Hidden only
          when the runtime can't be provisioned at all (nothing to track). */}
      {!unavailable && (
        <div className="flex flex-col" style={{ gap: 10, width: 250, margin: working ? '16px 0 2px' : '12px 0 2px', textAlign: 'left' }}>
          <ProvisionStep state={installStepState} label="Install on-device engine" />
          <ProvisionStep state={modelStepState} label="Download private models" />
        </div>
      )}

      {/* Status line — reflects the live phase / outcome */}
      <p style={{
        fontSize: 13, lineHeight: 1.5, maxWidth: 300, marginTop: working ? 18 : 4,
        color: wiz.modelReady ? 'var(--t-title)' : 'var(--t-muted)', textWrap: 'pretty',
      }}>
        {wiz.modelReady
          ? 'Your on-device engine is ready.'
          : unavailable
            ? 'The on-device runtime isn’t available for this Mac.'
            : showErr
              ? (wiz.err || 'The download didn’t finish.')
              : 'Downloading the models that run privately on your Mac — just once.'}
      </p>

      {/* Recoverable error → show the likely causes the user can self-check
          (most-likely first) plus a retry. Hidden for `unavailable` (a hardware
          verdict, not a fixable network issue). */}
      {showErr && !unavailable && (
        <div className="flex flex-col items-center" style={{ marginTop: 16 }}>
          <div style={{
            width: 330, textAlign: 'left', background: 'var(--t-box)',
            border: '0.5px solid var(--t-card-border)', borderRadius: 10, padding: '12px 14px',
          }}>
            <p className="font-mono" style={{ fontSize: 9.5, letterSpacing: '.1em', color: 'var(--t-faint)', marginBottom: 9 }}>
              IF IT KEEPS FAILING, CHECK
            </p>
            <div className="flex flex-col" style={{ gap: 9 }}>
              {SETUP_NET_HINTS.map((h, i) => (
                <div key={h.t} className="flex items-start" style={{ gap: 8 }}>
                  <span className="font-mono shrink-0" style={{ fontSize: 10.5, color: 'var(--t-faint-2)', fontWeight: 600, lineHeight: 1.55, width: 11 }}>{i + 1}</span>
                  <p style={{ fontSize: 11.5, lineHeight: 1.45, color: 'var(--t-muted)' }}>
                    <span style={{ fontWeight: 500, color: 'var(--t-title)' }}>{h.t}</span> — {h.d}
                  </p>
                </div>
              ))}
            </div>
          </div>
          <Btn size="sm" variant="secondary" onClick={wiz.retryModel} style={{ marginTop: 14 }}>Try again</Btn>
        </div>
      )}
    </div>
  )
}

// ── STEP 3 — Integrations ─────────────────────────────────────────────────────
// The whole connect surface is the shared <ConnectTrackers> (same component the
// dashboard uses), so all 5 providers + every connect flow live in one place.
function IntegrationsBody({ wiz }: { wiz: Wiz }) {
  const connected = TRACKERS.filter((t) => wiz.integrations?.[t.id]).length
  return (
    <div className="flex flex-col" style={{ gap: 9 }}>
      <ConnectTrackers integrations={wiz.integrations} onChanged={wiz.refetchIntegrations} compact />
      <p className="flex items-center" style={{ gap: 7, fontSize: 11, color: 'var(--t-faint)', marginTop: 3 }}>
        <span style={{ width: 5, height: 5, borderRadius: 99, background: connected ? 'var(--color-state-approved)' : 'var(--t-faint-2)' }} />
        {connected > 0
          ? `${connected} connected · Meridian will match sessions and draft worklogs.`
          : 'Connect your trackers to auto-draft worklogs — or skip and add later from Settings.'}
      </p>
    </div>
  )
}

// ── Welcome (pre-step intro) ──────────────────────────────────────────────────
export function Welcome({ onBegin }: { onBegin: () => void }) {
  const points = [
    { t: 'On-device', d: 'Runs on Apple MLX. Classifier input stays local unless you explicitly connect your tools.' },
    { t: 'Automatic', d: 'Recognises your work and drafts worklogs you approve.' },
    { t: 'Connected', d: 'Jira and Trello today, more trackers soon.' },
  ]
  return (
    <div className="flex flex-col items-center justify-center" style={{ height: '100%', textAlign: 'center', padding: '36px 44px' }}>
      <div className="flex items-center mer-pop" style={{ gap: 9, marginBottom: 24 }}>
        <span style={{ width: 9, height: 9, borderRadius: 99, background: 'var(--color-state-proposal)' }} />
        <span style={{ ...SERIF, fontSize: 25, lineHeight: 1, letterSpacing: '.01em', color: 'var(--t-title)' }}>meridian</span>
      </div>
      <Kicker style={{ marginBottom: 14 }}>First-run setup</Kicker>
      <h1 style={{ ...SERIF, fontSize: 39, lineHeight: 1.02, letterSpacing: '-.015em', color: 'var(--t-title)', maxWidth: 440, textWrap: 'balance' }}>
        Meridian watches the work, <span style={{ fontStyle: 'italic', color: 'var(--color-state-proposal)' }}>so you don&apos;t have to.</span>
      </h1>
      <p style={{ fontSize: 13.5, lineHeight: 1.55, color: 'var(--t-muted)', marginTop: 14, maxWidth: 380, textWrap: 'pretty' }}>
        A quiet menu-bar companion that recognises what you&apos;re focused on, drafts your worklogs, and keeps every byte on your Mac.
      </p>
      <div className="flex flex-col" style={{ gap: 11, margin: '26px 0 28px', textAlign: 'left', width: '100%', maxWidth: 360 }}>
        {points.map((p) => (
          <div key={p.t} className="flex items-start" style={{ gap: 11 }}>
            <span className="flex items-center justify-center shrink-0" style={{ width: 19, height: 19, borderRadius: 99, background: 'color-mix(in srgb, var(--color-state-proposal) 12%, transparent)', marginTop: 1 }}>
              <Check size={12} color="var(--color-state-proposal)" w={2.2} />
            </span>
            <p style={{ fontSize: 12.5, lineHeight: 1.4, color: 'var(--t-muted)' }}>
              <span style={{ fontWeight: 500, color: 'var(--t-title)' }}>{p.t}.</span> {p.d}
            </p>
          </div>
        ))}
      </div>
      <Btn onClick={onBegin} style={{ padding: '11px 26px', fontSize: 13.5 }}>Get started</Btn>
      <p className="font-mono" style={{ fontSize: 10.5, letterSpacing: '.04em', color: 'var(--t-faint-2)', marginTop: 14 }}>Three quick steps · about a minute</p>
    </div>
  )
}

// ── Completion ────────────────────────────────────────────────────────────────
export function Completion({ wiz }: { wiz: Wiz }) {
  const connected = TRACKERS.filter((t) => wiz.integrations?.[t.id])
  const grantedCount = [wiz.perms.accessibility, wiz.perms.screen, wiz.perms.input].filter(Boolean).length
  const lines = [
    { k: 'Permissions', v: `${grantedCount} of 3 granted` },
    { k: 'Local model', v: 'Qwen3.5 2B · 2B · 4-bit' },
    { k: 'Footprint', v: `${fmtSize(MODEL_RAM_GB + APP.ramGB)} memory` },
    { k: 'Connected', v: connected.length ? connected.map((c) => c.name).join(', ') : 'None yet' },
  ]
  return (
    <div className="flex flex-col items-center" style={{ textAlign: 'center', padding: '8px 8px 0' }}>
      <span className="flex items-center justify-center mer-pop" style={{ width: 56, height: 56, borderRadius: 99, background: 'color-mix(in srgb, var(--color-state-proposal) 12%, transparent)', color: 'var(--color-state-proposal)', marginBottom: 18 }}>
        <Check size={28} color="var(--color-state-proposal)" w={2.2} />
      </span>
      <Kicker style={{ marginBottom: 10 }}>Setup complete</Kicker>
      <h1 style={{ ...SERIF, fontSize: 38, lineHeight: 1, letterSpacing: '-.01em', color: 'var(--t-title)', marginBottom: 10 }}>You&apos;re all set.</h1>
      <p style={{ fontSize: 13.5, lineHeight: 1.5, color: 'var(--t-muted)', maxWidth: 340, textWrap: 'pretty', marginBottom: 22 }}>
        Meridian is now tracking quietly in your menu bar — on-device, private, and matched to your work.
      </p>
      <div style={{ width: '100%', maxWidth: 360, border: '0.5px solid var(--t-card-border)', borderRadius: 13, overflow: 'hidden' }}>
        {lines.map((l, i) => (
          <div key={l.k} className="flex items-center justify-between" style={{ padding: '10px 14px', borderTop: i ? '1px solid var(--t-hair)' : 'none' }}>
            <span className="font-mono" style={{ fontSize: 10, letterSpacing: '.12em', textTransform: 'uppercase', color: 'var(--t-faint)' }}>{l.k}</span>
            <span style={{ fontSize: 12.5, color: 'var(--t-title)', fontWeight: 450 }}>{l.v}</span>
          </div>
        ))}
      </div>
    </div>
  )
}

// ── STEP META — order, labels, headers, gating, rail status ───────────────────
export interface StepMeta {
  id: string
  n: string
  label: string
  kicker: string
  title: string
  subtitle: string
  Body: (props: { wiz: Wiz }) => ReactNode
  status: (w: Wiz) => string
  canNext: (w: Wiz) => boolean
}

// Local intelligence is LAST on purpose: the model download starts the moment
// the wizard opens (see page.tsx), so it runs in the background while the user
// handles Permissions + Integrations and is usually done — or nearly so — by the
// time they arrive here. Permissions stays first (capture needs them); the
// download-gated step sits at the end so the wait, if any, is the last thing.
export const STEPS: StepMeta[] = [
  {
    id: 'permissions', n: '01', label: 'Permissions', kicker: 'Access',
    title: 'Let Meridian see your work',
    subtitle: "Three macOS permissions let Meridian recognise what you're focused on. Read locally, never uploaded.",
    Body: PermissionsBody,
    status: (s) => { const g = [s.perms.accessibility, s.perms.screen, s.perms.input].filter(Boolean).length; return g ? `${g} granted` : 'Not granted' },
    canNext: (s) => !!(s.perms.accessibility && s.perms.screen && s.perms.input),
  },
  {
    id: 'integrations', n: '02', label: 'Integrations', kicker: 'Project tools',
    title: 'Connect your trackers',
    subtitle: 'Link the tools you already use. Meridian matches each session to an issue and drafts a worklog you approve.',
    Body: IntegrationsBody,
    status: (s) => { const c = TRACKERS.filter((t) => s.integrations?.[t.id]).length; return c ? `${c} connected` : 'Optional' },
    canNext: () => true,
  },
  {
    id: 'mlx', n: '03', label: 'Local intelligence', kicker: 'On-device AI',
    title: 'Set up on-device intelligence',
    subtitle: 'Everything runs privately on your Mac with Apple MLX. The models started downloading when setup opened — this is just the finish line.',
    Body: MLXBody,
    status: (s) => s.modelReady ? 'Ready' : (s.mlx?.runtime_found || s.mlx?.runtime_installed) ? 'Downloading…' : 'Installing…',
    // Block Finish until every model is on disk — the worklog pipeline (distill →
    // rerank → match) can't run a cycle without all three, so the user must not
    // reach the dashboard early. The download has had the whole wizard to run, so
    // this is usually instant; visible progress + Retry keep it from being a dead end.
    // Exception: if the runtime itself is unavailable (incompatible hardware, no
    // download_available), gate open — there is no download to wait for.
    canNext: (s) => s.modelReady || !!(s.mlx && !s.mlx.runtime_found && !s.mlx.runtime_installed && !s.mlx.download_available),
  },
]
