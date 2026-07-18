//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

// Static content + types for the first-run setup wizard ("A · Rail" shell,
// ported from the Meridian Setup design). Data only — no React, no side effects.
// Everything the wizard renders that ISN'T live machine state lives here.

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
    desc: 'Nudges you when a worklog draft is ready or your plan needs attention. Quiet by default - you control every type in Settings.',
  },
]

// Project-management integrations now live in the shared single source of truth
// `@/lib/integrations` (`TRACKERS`), rendered by the shared <ConnectTrackers>
// component in both the wizard (step 3) and the dashboard. The old wizard-only
// `INTEGRATIONS` list was removed in the centralisation.
