//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// Wizard step bodies + Welcome + Completion + the STEPS meta (ported from the
// design's steps.jsx). Every body is driven by the live `Wiz` handle built in
// page.tsx — permissions, MLX status, hardware specs, and OAuth state are all
// real. Nothing here fabricates data: the spec panel and memory gauge render
// detected hardware, and the model tiles map to real checkpoints.

import { useState } from 'react'
import type { CSSProperties, ReactNode } from 'react'
import { Btn, Bar, Check, Kicker, Mark, PermIcon, Row, Spinner } from './atoms'
import {
  APP, MODELS, MODEL_BY_ID, PERMISSIONS, fmtSize, recommendTier,
} from './data'
import type {
  DownloadProgress, MlxStatusResponse, ModelTier, SystemSpecs,
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
  // Step 2 — local intelligence (live MLX status + detected specs + model choice)
  specs: SystemSpecs | null
  mlx: MlxStatusResponse | null
  model: ModelTier['id']
  selectModel: (id: ModelTier['id']) => void
  downloading: boolean    // runtime tarball in flight
  prefetching: boolean    // model download in flight
  committing: boolean     // committed (Download clicked) but server not yet running
  modelReady: boolean
  progress: DownloadProgress | null
  installRuntime: () => void  // provision the MLX runtime
  downloadModel: () => void   // commit the chosen model, then prefetch it
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
              background: granted ? 'var(--accent-soft)' : 'var(--surface-2)',
              color: granted ? 'var(--accent)' : 'var(--ink-3)',
              border: '0.5px solid var(--rule-2)',
            }}><PermIcon icon={p.icon} /></span>

            <div style={{ flex: 1, minWidth: 0 }}>
              <div className="flex items-center" style={{ gap: 8 }}>
                <span style={{ fontSize: 13.5, fontWeight: 500, color: 'var(--ink)' }}>{p.name}</span>
                <span className="font-mono" style={{ fontSize: 9, letterSpacing: '.1em', color: 'var(--ink-3)', border: '0.5px solid var(--rule-2)', borderRadius: 4, padding: '1px 5px' }}>REQUIRED</span>
              </div>
              <p style={{ fontSize: 11.5, lineHeight: 1.4, color: 'var(--ink-3)', marginTop: 3 }}>{p.desc}</p>
            </div>

            <div className="shrink-0">
              {granted
                ? <span className="flex items-center" style={{ gap: 6, fontSize: 12, color: 'var(--success)', fontWeight: 500 }}><Check size={15} color="var(--success)" />Granted</span>
                : <Btn size="sm" variant="secondary" onClick={() => p.id === 'input' ? wiz.grantInput() : p.id === 'screen' ? wiz.grantScreen() : wiz.openPane(p.pane)}>Open Settings</Btn>}
            </div>
          </Row>
        )
      })}
      <p className="flex items-center" style={{ gap: 7, fontSize: 11, color: 'var(--ink-3)', marginTop: 3 }}>
        <span style={{ width: 5, height: 5, borderRadius: 99, background: 'var(--success)' }} />
        Everything is processed on your Mac. Meridian has no servers to send to.
      </p>
    </div>
  )
}

// ── STEP 2 — Local intelligence ──────────────────────────────────────────────
function SpecCell({ v, l, b }: { v: string | number; l: string; b?: boolean }) {
  return (
    <div style={{ padding: '11px 14px', borderRight: b ? '1px solid var(--rule)' : 'none' }}>
      <p className="font-mono tnum" style={{ fontSize: 19, fontWeight: 500, letterSpacing: '-.02em', color: 'var(--ink)', lineHeight: 1 }}>{v}</p>
      <p style={{ fontSize: 9.5, letterSpacing: '.08em', textTransform: 'uppercase', color: 'var(--ink-3)', marginTop: 6 }}>{l}</p>
    </div>
  )
}
function FootStat({ v, l, b }: { v: string; l: string; b?: boolean }) {
  return (
    <div style={{ padding: '10px 14px', borderRight: b ? '1px solid var(--rule)' : 'none' }}>
      <p className="font-mono tnum" style={{ fontSize: 15, fontWeight: 500, letterSpacing: '-.01em', color: 'var(--ink)', lineHeight: 1 }}>{v}</p>
      <p style={{ fontSize: 9.5, letterSpacing: '.08em', textTransform: 'uppercase', color: 'var(--ink-3)', marginTop: 5 }}>{l}</p>
    </div>
  )
}
function MemoryGauge({ model, app, total }: { model: number; app: number; total: number }) {
  const size = 132, sw = 13
  const cx = size / 2, cy = size / 2
  const r = (size - sw) / 2 - 1
  const C = 2 * Math.PI * r
  const sweep = 0.72
  const draw = sweep * C
  const start = 270 - sweep * 180
  const free = Math.max(0, total - model - app)
  const modelLen = Math.max(0, (model / total) * draw)
  const appLen = Math.max((app / total) * draw, app > 0 ? 3 : 0)
  return (
    <div style={{ position: 'relative', width: size, height: size, flexShrink: 0 }}>
      <svg width={size} height={size} style={{ display: 'block' }}>
        <g transform={`rotate(${start} ${cx} ${cy})`} fill="none" strokeLinecap="round">
          <circle cx={cx} cy={cy} r={r} stroke="var(--rule)" strokeWidth={sw} strokeDasharray={`${draw} ${C}`} />
          <circle cx={cx} cy={cy} r={r} stroke="#7C3AED" strokeWidth={sw} strokeDasharray={`${appLen} ${C}`} strokeDashoffset={`${-modelLen}`} style={{ transition: 'stroke-dasharray .4s, stroke-dashoffset .4s' }} />
          <circle cx={cx} cy={cy} r={r} stroke="var(--accent)" strokeWidth={sw} strokeDasharray={`${modelLen} ${C}`} style={{ transition: 'stroke-dasharray .4s' }} />
        </g>
      </svg>
      <div className="flex flex-col items-center justify-center" style={{ position: 'absolute', inset: 0 }}>
        <span className="font-mono tnum" style={{ fontSize: 25, fontWeight: 500, letterSpacing: '-.02em', lineHeight: 1, color: 'var(--ink)' }}>
          {free.toFixed(1)}<span style={{ fontSize: 11, color: 'var(--ink-3)', marginLeft: 2 }}>GB</span>
        </span>
        <span style={{ fontSize: 9.5, letterSpacing: '.1em', textTransform: 'uppercase', color: 'var(--ink-3)', marginTop: 5 }}>free of {total} GB</span>
      </div>
    </div>
  )
}
function GaugeLegend({ color, label, sub, val }: { color: string; label: string; sub: string; val: string }) {
  return (
    <div className="flex items-center" style={{ gap: 9 }}>
      <span style={{ width: 9, height: 9, borderRadius: 3, background: color, flexShrink: 0 }} />
      <div style={{ flex: 1, minWidth: 0 }}>
        <p style={{ fontSize: 12, color: 'var(--ink)', fontWeight: 450, lineHeight: 1.2 }}>{label}</p>
        <p style={{ fontSize: 10, color: 'var(--ink-4)', marginTop: 1 }}>{sub}</p>
      </div>
      <span className="font-mono tnum" style={{ fontSize: 12, color: 'var(--ink-2)', flexShrink: 0 }}>{val}</span>
    </div>
  )
}

function MLXBody({ wiz }: { wiz: Wiz }) {
  const [picking, setPicking] = useState(false)
  const sel = MODEL_BY_ID[wiz.model]
  const specs = wiz.specs
  const ram = specs?.ram_gb ?? 0
  const m = wiz.mlx
  const runtimeInstalled = !!(m && (m.runtime_found || m.runtime_installed))
  const machineRec = recommendTier(ram)
  const pct = wiz.progress && wiz.progress.total > 0 ? Math.round(wiz.progress.received / wiz.progress.total * 100) : null

  const memModel = sel.ramGB, memApp = APP.ramGB
  const memFree = Math.max(0, ram - memModel - memApp)
  const tight = ram > 0 && memModel + memApp > ram * 0.85

  return (
    <div className="flex flex-col" style={{ gap: 14 }}>
      {/* YOUR MAC — detected specs */}
      <div>
        <Kicker style={{ marginBottom: 7 }}>Your Mac</Kicker>
        <div style={{ border: '0.5px solid var(--rule-2)', borderRadius: 13, overflow: 'hidden', background: 'var(--surface)' }}>
          <div className="flex items-center" style={{ gap: 11, padding: '12px 15px', borderBottom: '1px solid var(--rule)' }}>
            <Mark mono="M" color="#7C3AED" size={30} />
            <div style={{ flex: 1, minWidth: 0 }}>
              <p style={{ fontSize: 13.5, fontWeight: 500, color: 'var(--ink)' }}>{specs?.chip || 'Detecting your Mac…'}</p>
              <p className="font-mono" style={{ fontSize: 10.5, color: 'var(--ink-3)', marginTop: 1 }}>{specs?.macos || '—'}</p>
            </div>
            {specs && (specs.chip || ram > 0) && (
              <span className="flex items-center" style={{ gap: 5, fontSize: 10.5, color: 'var(--success)' }}>
                <Check size={13} color="var(--success)" />Detected
              </span>
            )}
          </div>
          <div style={{ display: 'grid', gridTemplateColumns: 'repeat(4, 1fr)' }}>
            <SpecCell v={specs?.cpu_cores || '—'} l="CPU cores" b />
            <SpecCell v={specs?.gpu_cores || '—'} l="GPU cores" b />
            <SpecCell v={ram ? `${ram} GB` : '—'} l="Unified memory" b />
            <SpecCell v={specs?.free_disk_gb ? `${specs.free_disk_gb} GB` : '—'} l="Free storage" />
          </div>
        </div>
      </div>

      {/* RUNTIME */}
      <div>
        <Kicker style={{ marginBottom: 7 }}>Runtime</Kicker>
        <Row tone={runtimeInstalled ? 'tint' : 'surface'}>
          <Mark mono="MLX" color="#7C3AED" size={34} />
          <div style={{ flex: 1, minWidth: 0 }}>
            <p style={{ fontSize: 13.5, fontWeight: 500, color: 'var(--ink)' }}>Apple MLX runtime</p>
            <p className="font-mono" style={{ fontSize: 11, color: 'var(--ink-3)', marginTop: 2 }}>
              {specs?.gpu_cores ? `Accelerated on the ${specs.gpu_cores}-core GPU` : 'GPU-accelerated inference'}
            </p>
          </div>
          <div className="shrink-0" style={{ minWidth: 96, textAlign: 'right' }}>
            {runtimeInstalled
              ? <span className="flex items-center justify-end" style={{ gap: 6, fontSize: 12, color: 'var(--success)', fontWeight: 500 }}><Check size={15} color="var(--success)" />Installed</span>
              : wiz.downloading
                ? <span className="flex items-center justify-end font-mono tnum" style={{ gap: 7, fontSize: 11.5, color: 'var(--ink-2)' }}><Spinner />{pct !== null ? `${pct}%` : '…'}</span>
                : m && m.download_available
                  ? <Btn size="sm" variant="secondary" onClick={wiz.installRuntime}>Install</Btn>
                  : <span className="font-mono" style={{ fontSize: 11, color: 'var(--warn)' }}>Unavailable</span>}
          </div>
        </Row>
        {wiz.downloading && pct !== null && (
          <div style={{ marginTop: 8 }}><Bar pct={pct} height={4} /></div>
        )}
      </div>

      {/* MODEL — interactive once the runtime is present; until then the picker
          is dimmed (the server must be up before a model can be fetched). */}
      <div style={{ opacity: runtimeInstalled ? 1 : 0.45, pointerEvents: runtimeInstalled ? 'auto' : 'none', transition: 'opacity .25s' }}>
        <div className="flex items-center justify-between" style={{ marginBottom: 7 }}>
          <Kicker>Model</Kicker>
          {!picking && !wiz.modelReady && !wiz.prefetching && !wiz.committing && (
            <button onClick={() => setPicking(true)} style={{ fontSize: 11, color: 'var(--ink-3)' }}
              onMouseEnter={(e) => e.currentTarget.style.color = 'var(--ink)'}
              onMouseLeave={(e) => e.currentTarget.style.color = 'var(--ink-3)'}>Change model</button>
          )}
        </div>

        {picking ? (
          <div className="flex flex-col" style={{ gap: 7 }}>
            {MODELS.map((mod) => {
              const on = mod.id === wiz.model
              return (
                <button key={mod.id} onClick={() => { wiz.selectModel(mod.id); setPicking(false) }}
                  style={{ textAlign: 'left', display: 'flex', alignItems: 'center', gap: 12, padding: '11px 13px',
                    borderRadius: 12, border: `0.5px solid ${on ? 'var(--accent)' : 'var(--rule-2)'}`,
                    background: on ? 'var(--tint)' : 'var(--surface)', transition: 'all .12s' }}>
                  <span className="flex items-center justify-center shrink-0" style={{ width: 16, height: 16, borderRadius: 99, border: `1.5px solid ${on ? 'var(--accent)' : 'var(--rule-2)'}` }}>
                    {on && <span style={{ width: 8, height: 8, borderRadius: 99, background: 'var(--accent)' }} />}
                  </span>
                  <div style={{ flex: 1, minWidth: 0 }}>
                    <div className="flex items-center" style={{ gap: 8 }}>
                      <span style={{ fontSize: 13, fontWeight: 500, color: 'var(--ink)' }}>{mod.label}</span>
                      <span className="font-mono" style={{ fontSize: 10.5, color: 'var(--ink-3)' }}>{mod.model} · {mod.spec}</span>
                      {mod.id === machineRec && <span className="font-mono" style={{ fontSize: 8.5, letterSpacing: '.08em', color: 'var(--accent)', background: 'var(--accent-soft)', padding: '1px 5px', borderRadius: 4 }}>BEST FOR YOU</span>}
                    </div>
                    <p style={{ fontSize: 11, color: 'var(--ink-3)', marginTop: 2 }}>{mod.blurb}</p>
                  </div>
                  <span className="font-mono tnum shrink-0" style={{ fontSize: 11, color: 'var(--ink-2)' }}>{mod.ramGB} GB RAM</span>
                </button>
              )
            })}
          </div>
        ) : (
          <Row tone={wiz.modelReady ? 'tint' : 'surface'}>
            <Mark mono="m" color="var(--accent)" size={34} />
            <div style={{ flex: 1, minWidth: 0 }}>
              <div className="flex items-center" style={{ gap: 8 }}>
                <span style={{ fontSize: 13.5, fontWeight: 500, color: 'var(--ink)' }}>{sel.label}</span>
                <span className="font-mono" style={{ fontSize: 10.5, color: 'var(--ink-3)' }}>{sel.model} · {sel.spec}</span>
                {sel.recommended && <span className="font-mono" style={{ fontSize: 8.5, letterSpacing: '.08em', color: 'var(--accent)', background: 'var(--accent-soft)', padding: '1px 5px', borderRadius: 4 }}>RECOMMENDED</span>}
              </div>
              {(wiz.prefetching || wiz.committing) && !wiz.modelReady ? (
                <p className="font-mono tnum" style={{ fontSize: 11, color: 'var(--ink-3)', marginTop: 3 }}>
                  {wiz.progress?.message ?? 'Downloading model…'}
                </p>
              ) : (
                <p className="font-mono" style={{ fontSize: 11, color: 'var(--ink-3)', marginTop: 2 }}>{sel.sizeGB} GB download · {sel.speed} on your Mac</p>
              )}
            </div>
            <div className="shrink-0" style={{ minWidth: 100, textAlign: 'right' }}>
              {wiz.modelReady
                ? <span className="flex items-center justify-end" style={{ gap: 6, fontSize: 12, color: 'var(--success)', fontWeight: 500 }}><Check size={15} color="var(--success)" />Ready</span>
                : wiz.committing
                  ? <span className="flex items-center justify-end" style={{ gap: 7, fontSize: 11.5, color: 'var(--ink-2)' }}><Spinner />Starting…</span>
                  : wiz.prefetching
                    ? <span className="font-mono tnum" style={{ fontSize: 13, color: 'var(--accent)', fontWeight: 600 }}>{pct !== null ? `${pct}%` : '…'}</span>
                    : <Btn size="sm" onClick={wiz.downloadModel}>Download</Btn>}
            </div>
          </Row>
        )}
        {wiz.prefetching && !wiz.modelReady && !picking && pct !== null && (
          <div style={{ marginTop: 8 }}><Bar pct={pct} /></div>
        )}
      </div>

      {/* FOOTPRINT — real, against detected memory */}
      {ram > 0 && (
        <div>
          <div className="flex items-center justify-between" style={{ marginBottom: 7 }}>
            <Kicker>Estimated footprint</Kicker>
            <span className="flex items-center font-mono" style={{ gap: 5, fontSize: 10, color: tight ? 'var(--warn)' : 'var(--success)' }}>
              <span style={{ width: 5, height: 5, borderRadius: 99, background: tight ? 'var(--warn)' : 'var(--success)' }} />
              {tight ? 'Heavy for this Mac' : 'Comfortable fit'}
            </span>
          </div>
          <div style={{ border: '0.5px solid var(--rule-2)', borderRadius: 13, overflow: 'hidden', background: 'var(--surface)' }}>
            <div className="flex items-center" style={{ gap: 18, padding: '16px 18px' }}>
              <MemoryGauge model={memModel} app={memApp} total={ram} />
              <div className="flex flex-col" style={{ flex: 1, minWidth: 0, gap: 11 }}>
                <GaugeLegend color="var(--accent)" label={sel.label} sub={`${sel.model} · ${sel.spec}`} val={fmtSize(memModel)} />
                <GaugeLegend color="#7C3AED" label="Meridian app" sub="background service" val={fmtSize(memApp)} />
                <GaugeLegend color="var(--rule-2)" label="Free for everything else" sub={`${Math.round(memFree / ram * 100)}% of memory`} val={fmtSize(memFree)} />
              </div>
            </div>
            <div style={{ display: 'grid', gridTemplateColumns: 'repeat(3, 1fr)', borderTop: '1px solid var(--rule)' }}>
              <FootStat v={fmtSize(sel.sizeGB + APP.diskGB)} l="Install size" b />
              <FootStat v={specs?.free_disk_gb ? fmtSize(specs.free_disk_gb - sel.sizeGB - APP.diskGB) : '—'} l="Disk free after" b />
              <FootStat v={sel.speed} l="Inference" />
            </div>
          </div>
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
      <p className="flex items-center" style={{ gap: 7, fontSize: 11, color: 'var(--ink-3)', marginTop: 3 }}>
        <span style={{ width: 5, height: 5, borderRadius: 99, background: connected ? 'var(--success)' : 'var(--ink-4)' }} />
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
        <span style={{ width: 9, height: 9, borderRadius: 99, background: 'var(--accent)' }} />
        <span style={{ ...SERIF, fontSize: 25, lineHeight: 1, letterSpacing: '.01em', color: 'var(--ink)' }}>meridian</span>
      </div>
      <Kicker style={{ marginBottom: 14 }}>First-run setup</Kicker>
      <h1 style={{ ...SERIF, fontSize: 39, lineHeight: 1.02, letterSpacing: '-.015em', color: 'var(--ink)', maxWidth: 440, textWrap: 'balance' }}>
        Meridian watches the work, <span style={{ fontStyle: 'italic', color: 'var(--accent)' }}>so you don&apos;t have to.</span>
      </h1>
      <p style={{ fontSize: 13.5, lineHeight: 1.55, color: 'var(--ink-2)', marginTop: 14, maxWidth: 380, textWrap: 'pretty' }}>
        A quiet menu-bar companion that recognises what you&apos;re focused on, drafts your worklogs, and keeps every byte on your Mac.
      </p>
      <div className="flex flex-col" style={{ gap: 11, margin: '26px 0 28px', textAlign: 'left', width: '100%', maxWidth: 360 }}>
        {points.map((p) => (
          <div key={p.t} className="flex items-start" style={{ gap: 11 }}>
            <span className="flex items-center justify-center shrink-0" style={{ width: 19, height: 19, borderRadius: 99, background: 'var(--accent-soft)', marginTop: 1 }}>
              <Check size={12} color="var(--accent)" w={2.2} />
            </span>
            <p style={{ fontSize: 12.5, lineHeight: 1.4, color: 'var(--ink-2)' }}>
              <span style={{ fontWeight: 500, color: 'var(--ink)' }}>{p.t}.</span> {p.d}
            </p>
          </div>
        ))}
      </div>
      <Btn onClick={onBegin} style={{ padding: '11px 26px', fontSize: 13.5 }}>Get started</Btn>
      <p className="font-mono" style={{ fontSize: 10.5, letterSpacing: '.04em', color: 'var(--ink-4)', marginTop: 14 }}>Three quick steps · about a minute</p>
    </div>
  )
}

// ── Completion ────────────────────────────────────────────────────────────────
export function Completion({ wiz }: { wiz: Wiz }) {
  const connected = TRACKERS.filter((t) => wiz.integrations?.[t.id])
  const model = MODEL_BY_ID[wiz.model]
  const grantedCount = [wiz.perms.accessibility, wiz.perms.screen, wiz.perms.input].filter(Boolean).length
  const lines = [
    { k: 'Permissions', v: `${grantedCount} of 3 granted` },
    { k: 'Local model', v: `${model.label} · ${model.model}` },
    { k: 'Footprint', v: `${fmtSize(model.ramGB + APP.ramGB)} memory` },
    { k: 'Connected', v: connected.length ? connected.map((c) => c.name).join(', ') : 'None yet' },
  ]
  return (
    <div className="flex flex-col items-center" style={{ textAlign: 'center', padding: '8px 8px 0' }}>
      <span className="flex items-center justify-center mer-pop" style={{ width: 56, height: 56, borderRadius: 99, background: 'var(--accent-soft)', color: 'var(--accent)', marginBottom: 18 }}>
        <Check size={28} color="var(--accent)" w={2.2} />
      </span>
      <Kicker style={{ marginBottom: 10 }}>Setup complete</Kicker>
      <h1 style={{ ...SERIF, fontSize: 38, lineHeight: 1, letterSpacing: '-.01em', color: 'var(--ink)', marginBottom: 10 }}>You&apos;re all set.</h1>
      <p style={{ fontSize: 13.5, lineHeight: 1.5, color: 'var(--ink-2)', maxWidth: 340, textWrap: 'pretty', marginBottom: 22 }}>
        Meridian is now tracking quietly in your menu bar — on-device, private, and matched to your work.
      </p>
      <div style={{ width: '100%', maxWidth: 360, border: '0.5px solid var(--rule-2)', borderRadius: 13, overflow: 'hidden' }}>
        {lines.map((l, i) => (
          <div key={l.k} className="flex items-center justify-between" style={{ padding: '10px 14px', borderTop: i ? '1px solid var(--rule)' : 'none' }}>
            <span className="font-mono" style={{ fontSize: 10, letterSpacing: '.12em', textTransform: 'uppercase', color: 'var(--ink-3)' }}>{l.k}</span>
            <span style={{ fontSize: 12.5, color: 'var(--ink)', fontWeight: 450 }}>{l.v}</span>
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
    id: 'mlx', n: '02', label: 'Local intelligence', kicker: 'On-device AI',
    title: 'Install the on-device engine',
    subtitle: "Meridian runs entirely on your Mac with Apple MLX. We've sized a model to your hardware — here's exactly what it will use.",
    Body: MLXBody,
    status: (s) => s.modelReady ? 'Ready' : (s.mlx?.runtime_found || s.mlx?.runtime_installed) ? 'Model pending' : 'Not installed',
    // Never trap the user: the model finishes downloading in the background.
    canNext: () => true,
  },
  {
    id: 'integrations', n: '03', label: 'Integrations', kicker: 'Project tools',
    title: 'Connect your trackers',
    subtitle: 'Link the tools you already use. Meridian matches each session to an issue and drafts a worklog you approve.',
    Body: IntegrationsBody,
    status: (s) => { const c = TRACKERS.filter((t) => s.integrations?.[t.id]).length; return c ? `${c} connected` : 'Optional' },
    canNext: () => true,
  },
]
