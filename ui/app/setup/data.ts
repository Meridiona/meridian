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

// ── On-device model ──────────────────────────────────────────────────────────
// Meridian uses a single fixed classifier model. No user selection.

export const MODEL_ID = 'mlx-community/Qwen3.5-2B-OptiQ-4bit'
export const MODEL_SIZE_GB = 1.2
export const MODEL_RAM_GB = 1.5

/** Meridian's own resident footprint (background service), separate from the model. */
export const APP = { diskGB: 0.18, ramGB: 0.15 }

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
