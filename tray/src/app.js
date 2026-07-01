//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
/* global __TAURI__ */
'use strict'
// Rebuilt from the Claude Design mock "Meridian Tray.dc.html"
// (claude.ai/design/p/b8656e29-ae04-4f69-b17f-d5fab4d00f3a).

const invoke = (cmd, args) => __TAURI__.core.invoke(cmd, args)
const listen = (evt, cb) => __TAURI__.event.listen(evt, cb)

// Forward JS exceptions to the tray's stderr log (no devtools in a packaged
// build). Diagnostic only — safe to keep.
const dbg = (msg) => { try { invoke('tray_debug', { msg: String(msg) }) } catch {} }
window.onerror = (m, src, line, col) => dbg(`popover onerror: ${m} @${line}:${col}`)
window.addEventListener('unhandledrejection', (e) => dbg(`popover rejection: ${e.reason}`))

// ── Elements ─────────────────────────────────────────────────────────────────
const $ = (id) => document.getElementById(id)
const brandMark = $('brand-mark')
const headSub = $('head-sub')
const statusCard = $('status-card')
const statusDot = $('status-dot')
const statusTitle = $('status-title')
const statusSub = $('status-sub')
const primaryBtn = $('primary-btn')
const pauseOptions = $('pause-options')
const optHints = {
  15: $('opt-15m-hint'),
  60: $('opt-1h-hint'),
  tomorrow: $('opt-tomorrow-hint'),
}
const pauseActive = $('pause-active')
const pauseCountdown = $('pause-countdown')
const resumeBtn = $('resume-btn')
const statLogged = $('stat-logged')
const statFocus = $('stat-focus')
const statDrafts = $('stat-drafts')
const reviewCta = $('review-cta')
const reviewCount = $('review-count')
const upd = $('upd')
const updText = $('upd-text')
const updVer = $('upd-ver')

// ── State for the local 1-second ticker ──────────────────────────────────────
let elapsed = 0        // live session seconds; re-synced on every status payload
let isTracking = false // healthy + has an active session, not paused
let activeAppName = ''
let tickId = null

// ── Pause state (synced from StatusPayload) ───────────────────────────────────
let pauseUntilMs = null     // ms timestamp when timed pause ends; null = not timed-paused
let countdownId = null      // interval handle for the live countdown
let daemonUnhealthy = false // true when healthy=false; drives primaryBtn's action
let optionsOpen = false     // pause-duration list expanded (mirrors the mock's showOptions)

// ── Formatters ───────────────────────────────────────────────────────────────
// "6h 30m" / "30m" / "0m" — zero-pads minutes only when hours are present.
function fmtTile(secs) {
  const m = Math.floor((secs || 0) / 60)
  const h = Math.floor(m / 60)
  const rm = m % 60
  if (h === 0) return `${rm}m`
  return `${h}h ${String(rm).padStart(2, '0')}m`
}

function clockOf(date) {
  let h = date.getHours()
  const m = date.getMinutes()
  const period = h >= 12 ? 'PM' : 'AM'
  h = ((h + 11) % 12) + 1
  return `${h}:${String(m).padStart(2, '0')} ${period}`
}

// Seconds from now until 9:00 AM the following calendar day (the "Pause until
// tomorrow" preset — matches the design mock's fixed morning resume time).
function secsUntilTomorrowMorning() {
  const target = new Date()
  target.setDate(target.getDate() + 1)
  target.setHours(9, 0, 0, 0)
  return Math.max(60, Math.round((target.getTime() - Date.now()) / 1000))
}

// ── Countdown rendering ───────────────────────────────────────────────────────
// fmtCountdown / parsePauseMins / pauseLabel are defined in pause-utils.js
// (loaded first) so they can be tested in isolation.

function paintCountdown() {
  if (!pauseUntilMs) return
  const remain = pauseUntilMs - Date.now()
  pauseCountdown.textContent = fmtCountdown(remain)
}

function startCountdown(untilMs) {
  pauseUntilMs = untilMs
  paintCountdown()
  if (countdownId) clearInterval(countdownId)
  countdownId = setInterval(() => {
    paintCountdown()
    if (Date.now() >= pauseUntilMs) stopCountdown()
  }, 1000)
}

function stopCountdown() {
  if (countdownId) clearInterval(countdownId)
  countdownId = null
  pauseUntilMs = null
}

// ── Ticker (keeps the "X min in" subtitle fresh between polls) ───────────────
function paintTrackingSub() {
  if (!isTracking) return
  statusSub.textContent = activeAppName
    ? `This hour · ${fmtTile(elapsed)} in · ${activeAppName}`
    : `This hour · ${fmtTile(elapsed)} in`
}

function startTicker() {
  if (tickId) return
  tickId = setInterval(() => {
    if (!isTracking) return
    elapsed += 1
    paintTrackingSub()
  }, 1000)
}

// Refresh the "resumes at …" hints whenever the duration list is shown, since
// they're wall-clock computed from "now".
function paintOptionHints() {
  optHints[15].textContent = clockOf(new Date(Date.now() + 15 * 60 * 1000))
  optHints[60].textContent = clockOf(new Date(Date.now() + 60 * 60 * 1000))
  const tomorrow = new Date()
  tomorrow.setDate(tomorrow.getDate() + 1)
  tomorrow.setHours(9, 0, 0, 0)
  optHints.tomorrow.textContent = clockOf(tomorrow)
}

// ── Render ───────────────────────────────────────────────────────────────────
function render(status) {
  const healthy = status.ui_reachable && status.healthy
  const hasActive = !!status.active_app
  const pauseSource = status.pause_source || null // null | 'timed' | 'schedule' | 'indefinite'
  const isPaused = !!pauseSource

  brandMark.classList.toggle('down', !healthy)
  isTracking = healthy && hasActive && !isPaused
  activeAppName = status.active_app || ''
  elapsed = isTracking ? (status.active_elapsed_s || 0) : 0

  headSub.textContent = !status.has_polled
    ? 'Connecting…'
    : !healthy
      ? 'Offline'
      : isPaused
        ? 'Capture paused'
        : 'Capturing this hour'

  statusCard.classList.toggle('paused', !healthy || isPaused)
  stopCountdown()
  pauseActive.hidden = true
  pauseOptions.hidden = true
  primaryBtn.hidden = false

  if (!status.has_polled) {
    statusTitle.textContent = 'Connecting…'
    statusSub.textContent = ''
    primaryBtn.hidden = true
    daemonUnhealthy = false
  } else if (!healthy) {
    daemonUnhealthy = true
    statusTitle.textContent = 'Meridian is offline'
    statusSub.textContent = "Not recording — restart the daemon"
    primaryBtn.textContent = 'Restart daemon'
    primaryBtn.classList.remove('open')
  } else if (pauseSource === 'timed') {
    daemonUnhealthy = false
    statusTitle.textContent = 'Capture paused'
    statusSub.textContent = ''
    primaryBtn.hidden = true
    pauseActive.hidden = false
    pauseCountdown.hidden = false
    resumeBtn.textContent = 'Resume now'
    const untilMs = status.pause_until_ms || 0
    if (untilMs) startCountdown(untilMs)
  } else if (pauseSource === 'indefinite') {
    daemonUnhealthy = false
    statusTitle.textContent = 'Capture paused'
    statusSub.textContent = 'Paused until you resume'
    primaryBtn.hidden = true
    pauseActive.hidden = false
    pauseCountdown.hidden = true
    resumeBtn.textContent = 'Resume now'
  } else if (pauseSource === 'schedule') {
    daemonUnhealthy = false
    statusTitle.textContent = 'Outside work hours'
    const resumeAt = status.schedule_resume_at || ''
    statusSub.textContent = resumeAt ? `Resumes at ${resumeAt}` : 'Work hours not active'
    primaryBtn.hidden = true
  } else if (hasActive) {
    daemonUnhealthy = false
    statusTitle.textContent = 'Capturing your screen'
    paintTrackingSub()
    primaryBtn.textContent = optionsOpen ? 'Choose how long…' : 'Pause capture'
    primaryBtn.classList.toggle('open', optionsOpen)
    pauseOptions.hidden = !optionsOpen
  } else {
    daemonUnhealthy = false
    statusTitle.textContent = 'Nothing tracked right now'
    statusSub.textContent = 'Meridian is watching — nothing to track yet.'
    primaryBtn.textContent = optionsOpen ? 'Choose how long…' : 'Pause capture'
    primaryBtn.classList.toggle('open', optionsOpen)
    pauseOptions.hidden = !optionsOpen
  }
  if (optionsOpen && !pauseOptions.hidden) paintOptionHints()

  // ── Today stats ────────────────────────────────────────────────────────────
  statLogged.textContent = fmtTile(status.logged_s)
  statFocus.textContent = fmtTile(status.focus_s)
  statDrafts.textContent = String(status.drafts_count || 0)

  // ── Review CTA (real drafts only) ─────────────────────────────────────────
  const drafts = status.drafts_count || 0
  if (drafts > 0) {
    reviewCta.hidden = false
    reviewCount.textContent = String(drafts)
  } else {
    reviewCta.hidden = true
  }
}

// ── Events + actions ─────────────────────────────────────────────────────────
listen('status-update', (event) => { render(event.payload); resizeToContent() })

// Escape closes the popover. The popover runs as a non-activating NSPanel so
// Focused(false) never fires on macOS — Escape is the keyboard dismiss path.
document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') invoke('hide_popover').catch(() => {})
})

// open_dashboard / open_worklogs / open_permission_pane all dismiss the
// popover themselves server-side (see dismiss_popover in commands/system.rs)
// — no client-side "and also hide" call needed, and no race between two
// independent invoke()s.
$('btn-open-head').addEventListener('click', () => invoke('open_dashboard').catch(console.error))
$('btn-open').addEventListener('click', () => invoke('open_dashboard').catch(console.error))
$('btn-perms').addEventListener('click', () =>
  invoke('open_permission_pane', { pane: 'screen_recording' }).catch(console.error))
$('btn-quit').addEventListener('click', () => invoke('quit_app').catch(console.error))
reviewCta.addEventListener('click', () => invoke('open_worklogs').catch(console.error))

// Primary button: restart the daemon when unhealthy, otherwise toggle the
// pause-duration list open/closed (mirrors the design's togglePauseMenu).
primaryBtn.addEventListener('click', () => {
  if (daemonUnhealthy) {
    invoke('restart_daemon').catch(console.error)
    return
  }
  optionsOpen = !optionsOpen
  pauseOptions.hidden = !optionsOpen
  primaryBtn.textContent = optionsOpen ? 'Choose how long…' : 'Pause capture'
  primaryBtn.classList.toggle('open', optionsOpen)
  if (optionsOpen) paintOptionHints()
  resizeToContent()
})

$('opt-15m').addEventListener('click', () => {
  optionsOpen = false
  invoke('pause_for_duration', { seconds: 900 }).catch(console.error)
})
$('opt-1h').addEventListener('click', () => {
  optionsOpen = false
  invoke('pause_for_duration', { seconds: 3600 }).catch(console.error)
})
$('opt-tomorrow').addEventListener('click', () => {
  optionsOpen = false
  invoke('pause_for_duration', { seconds: secsUntilTomorrowMorning() }).catch(console.error)
})
$('opt-indefinite').addEventListener('click', () => {
  optionsOpen = false
  invoke('pause_indefinitely').catch(console.error)
})

// Resume now (timed/indefinite pause) or restart daemon (unhealthy) — the
// daemon-unhealthy case never shows pause-active (see render()), so this
// button's action is always "resume" in practice; the daemonUnhealthy guard
// is defence-in-depth only.
resumeBtn.addEventListener('click', () => {
  if (daemonUnhealthy) {
    invoke('restart_daemon').catch(console.error)
  } else {
    invoke('pause_for_duration', { seconds: 0 }).catch(console.error)
  }
})

// ── DMG auto-update ───────────────────────────────────────────────────────────
// Check on open; show the banner only when an update is actually available
// (state 'uptodate'/'unsupported'/'error' stay silent in the compact popover —
// the dashboard sidebar surfaces the diagnostic error text). Click → download +
// install + relaunch, with live progress from `update-progress` events.
let installing = false
function checkUpdate() {
  invoke('check_update').then((s) => {
    if (s && s.state === 'available' && s.version) {
      updVer.textContent = `v${s.currentVersion} → v${s.version}`
      upd.hidden = false
      resizeToContent()
    }
  }).catch(() => {})
}
listen('update-progress', (e) => {
  const d = e.payload || {}
  if (installing && d.contentLength) {
    updText.textContent = `Downloading… ${Math.round((d.downloaded / d.contentLength) * 100)}%`
  }
})
upd.addEventListener('click', () => {
  if (installing) return
  installing = true
  updText.textContent = 'Starting…'
  // Resolves only on failure — success re-execs the app (the relaunch).
  invoke('install_update').catch((err) => {
    installing = false
    updText.textContent = 'Update failed'
    dbg(`update install failed: ${err}`)
  })
})

// ── Window sizing ─────────────────────────────────────────────────────────────
// Resize the Tauri window to exactly match the card's rendered height so no
// transparent gap appears below the popup (a gap reads as a white band over the
// wallpaper) and the card's rounded bottom corners are never clipped by the
// window edge (which would make them look square + introduce a scrollbar).
const popEl = document.getElementById('pop')
let lastFitH = 0
function resizeToContent() {
  const h = Math.ceil(popEl.getBoundingClientRect().height)
  if (h < 100 || h === lastFitH) return // not painted yet, or already this tall
  lastFitH = h
  try {
    const { LogicalSize, getCurrentWindow } = window.__TAURI__.window
    getCurrentWindow().setSize(new LogicalSize(344, h))
      .then(() => dbg(`popover resize -> measured pop height=${h}`))
      .catch((e) => dbg(`popover setSize FAILED: ${e}`))
  } catch (e) { dbg(`popover resize threw: ${e}`) }
}

// Re-fit on ANY card height change — web-font swap (Plus Jakarta Sans / mono
// load after first paint and change metrics), the options list / review CTA
// showing/hiding, etc. Without this the window keeps its first (pre-font)
// height and the taller card overflows → clipped rounded bottom + scrollbar.
if (window.ResizeObserver) {
  new ResizeObserver(() => resizeToContent()).observe(popEl)
}
if (document.fonts && document.fonts.ready) {
  document.fonts.ready.then(() => resizeToContent()).catch(() => {})
}

// ── Boot ──────────────────────────────────────────────────────────────────────
startTicker()
invoke('get_status').then((s) => { render(s); resizeToContent() }).catch(() => {})
checkUpdate()
