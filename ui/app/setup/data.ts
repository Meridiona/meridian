//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

// Static content + types for the first-run setup wizard ("A · Rail" shell,
// ported from the Meridian Setup design). Data only — no React, no side effects.
// Everything the wizard renders that ISN'T live machine state lives here.

// ── Live backend response shapes (mirror tray/src-tauri/src/commands/setup.rs) ──

/** MLX server status — polled on the Model step (`get_mlx_status`). */
export type MlxStatus = 'offline' | 'starting' | 'running' | { error: string }

export interface MlxStatusResponse {
  status: MlxStatus
  port: number
  runtime_found: boolean
  runtime_installed: boolean
  download_available: boolean
}

/** Streamed by both the runtime download and the model prefetch. */
export interface DownloadProgress {
  received: number
  total: number
  message: string
}

/** Detected hardware (`detect_system_specs`). All-zero on non-macOS / probe failure. */
export interface SystemSpecs {
  chip: string
  macos: string
  cpu_cores: number
  gpu_cores: number
  ram_gb: number
  free_disk_gb: number
}

// ── On-device model tiers ────────────────────────────────────────────────────
// Each tier maps to a REAL MLX checkpoint in the llm_selector catalog
// (services/agents/llm_selector.py). The picker writes the chosen `hfId` to
// settings.json via `set_model_preference`; the server then prefers it. We show
// the real model identity (family · params · quant) — never invented names.
// `ramGB` is the resident working set from the catalog; `sizeGB` is the
// approximate 4-bit download size. Both feed the footprint gauge against the
// machine's real detected memory.

export interface ModelTier {
  id: 'nano' | 'core' | 'max'
  label: string      // friendly tier name shown prominently
  model: string      // real model identity (mono sub-line)
  spec: string       // params · quant
  hfId: string       // HuggingFace repo id persisted as the preference
  sizeGB: number     // approx download size
  ramGB: number      // resident working set (catalog min_ram_gb)
  speed: string      // rough tok/s on Apple Silicon
  recommended?: boolean
  blurb: string
}

export const MODELS: ModelTier[] = [
  {
    id: 'nano', label: 'Compact', model: 'Qwen3.5 4B', spec: '4B · 4-bit',
    hfId: 'mlx-community/Qwen3.5-4B-MLX-4bit',
    sizeGB: 2.2, ramGB: 2.5, speed: '~75 tok/s',
    blurb: 'Fastest and lightest. Great on 8 GB Macs.',
  },
  {
    id: 'core', label: 'Balanced', model: 'Qwen3.5 9B', spec: '9B · 4-bit',
    hfId: 'mlx-community/Qwen3.5-9B-OptiQ-4bit',
    sizeGB: 5.0, ramGB: 6.5, speed: '~48 tok/s', recommended: true,
    blurb: "Tuned for Meridian's classifier. Best balance for Apple Silicon.",
  },
  {
    id: 'max', label: 'Maximum', model: 'Phi-4 14B', spec: '14B · 4-bit',
    hfId: 'mlx-community/phi-4-4bit',
    sizeGB: 8.0, ramGB: 8.5, speed: '~30 tok/s',
    blurb: 'Highest accuracy. Needs 16 GB+ unified memory.',
  },
]

export const MODEL_BY_ID: Record<string, ModelTier> =
  Object.fromEntries(MODELS.map((m) => [m.id, m]))

/** Meridian's own resident footprint (background service), separate from the model. */
export const APP = { diskGB: 0.18, ramGB: 0.15 }

/**
 * The tier best suited to a machine's unified memory — drives the default
 * selection's "Best for you" badge. `core` is the classifier-tuned default and
 * the safe fallback when specs are unknown (ramGB === 0).
 */
export function recommendTier(ramGB: number): ModelTier['id'] {
  if (ramGB >= 32) return 'max'
  if (ramGB >= 16) return 'core'
  if (ramGB > 0) return 'nano'
  return 'core'
}

// ── macOS permissions — the three the backend actually requires + polls ───────
// (The design's Notifications + Launch-at-login toggles are intentionally
// omitted: no backend exists for them, and the in-process capture pipeline
// genuinely needs Input Monitoring, which the design omitted.)

export interface PermissionMeta {
  id: 'screen' | 'accessibility' | 'input'
  icon: 'screen' | 'access' | 'power'
  name: string
  pane: string      // open_permission_pane argument
  desc: string
}

export const PERMISSIONS: PermissionMeta[] = [
  {
    id: 'accessibility', icon: 'access', name: 'Accessibility', pane: 'accessibility',
    desc: 'Reads the active app, window titles, and UI labels for accurate context.',
  },
  {
    id: 'screen', icon: 'screen', name: 'Screen Recording', pane: 'screen_recording',
    desc: 'Reads on-screen text to understand your work. Pixels/video are never stored; extracted text stays on-device.',
  },
  {
    id: 'input', icon: 'power', name: 'Input Monitoring', pane: 'input_monitoring',
    desc: 'Detects clicks and typing so Meridian knows when you switch tasks.',
  },
]

// Project-management integrations now live in the shared single source of truth
// `@/lib/integrations` (`TRACKERS`), rendered by the shared <ConnectTrackers>
// component in both the wizard (step 3) and the dashboard. The old wizard-only
// `INTEGRATIONS` list was removed in the centralisation.

/** Whole-GB / MB size label, matching the design's `fmtSize`. */
export const fmtSize = (gb: number): string =>
  gb < 1 ? `${Math.round(gb * 1000)} MB` : gb < 100 ? `${gb.toFixed(1)} GB` : `${Math.round(gb)} GB`
