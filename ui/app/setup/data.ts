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
  /** Live transfer rate in bytes/sec (measured server-side by HF; 0 when idle/stalled). */
  speed: number
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

// ── On-device models ─────────────────────────────────────────────────────────
// Meridian runs a fixed set of three models — the Qwen3.5-2B generative LLM
// (which also does classification/matching), a reranker, and an embedder. They
// load one at a time (single-slot), so peak resident memory is the LLM
// (MODEL_RAM_GB). No user selection; the wizard auto-downloads the whole set and
// shows live aggregate progress.

export const MODEL_ID = 'mlx-community/Qwen3.5-2B-OptiQ-4bit'
export const MODEL_RAM_GB = 1.5

/** Meridian's own resident footprint (background service), separate from the model. */
export const APP = { diskGB: 0.18, ramGB: 0.15 }

// ── macOS permissions — two required TCC grants + optional notifications ─────
// (The design's Launch-at-login toggle is intentionally omitted: no backend
// exists for it. Input Monitoring is omitted too: the signals the daemon
// actually consumes — clipboard + app_switch (get_signals) — come from the
// Accessibility-only workspace observer + clipboard poller, so they need no
// separate Input Monitoring grant. Input Monitoring would only add the
// click/key/text CGEventTap, which feeds solely the minor Option-C `ended_at`
// refinement — not worth its own wizard step + TCC prompt. See
// screenpipe-a11y/src/platform/macos.rs — Thread 2 is "accessibility only".)

/** Notification authorization (`check_notifications`). Tri-state, not boolean:
 *  macOS shows the authorization dialog exactly once, so `denied` and `prompt`
 *  need different grant actions (Settings pane vs system dialog).
 *  `unavailable` = unbundled run (`tauri dev`) — the card hides itself. */
export type NotifState = 'granted' | 'denied' | 'prompt' | 'unavailable'

export interface PermissionMeta {
  id: 'screen' | 'accessibility' | 'notifications'
  icon: 'screen' | 'access' | 'bell'
  name: string
  pane: string      // open_permission_pane argument
  desc: string
  /** Required grants gate the step's Continue; optional ones never do. */
  required: boolean
}

export const PERMISSIONS: PermissionMeta[] = [
  {
    id: 'accessibility', icon: 'access', name: 'Accessibility', pane: 'accessibility', required: true,
    desc: 'Reads the active app, window titles, and UI labels for accurate context.',
  },
  {
    id: 'screen', icon: 'screen', name: 'Screen Recording', pane: 'screen_recording', required: true,
    desc: 'Reads on-screen text to understand your work. Pixels/video are never stored; extracted text stays on-device.',
  },
  {
    id: 'notifications', icon: 'bell', name: 'Notifications', pane: 'notifications', required: false,
    desc: 'Nudges you when a worklog draft is ready or your plan needs attention. Quiet by default — you control every type in Settings.',
  },
]

// Project-management integrations now live in the shared single source of truth
// `@/lib/integrations` (`TRACKERS`), rendered by the shared <ConnectTrackers>
// component in both the wizard (step 3) and the dashboard. The old wizard-only
// `INTEGRATIONS` list was removed in the centralisation.

/** Whole-GB / MB size label, matching the design's `fmtSize`. */
export const fmtSize = (gb: number): string =>
  gb < 1 ? `${Math.round(gb * 1000)} MB` : gb < 100 ? `${gb.toFixed(1)} GB` : `${Math.round(gb)} GB`
