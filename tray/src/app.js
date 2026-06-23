//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
/* global __TAURI__ */
'use strict'

const invoke = (cmd, args) => __TAURI__.core.invoke(cmd, args)
const listen = (evt, cb) => __TAURI__.event.listen(evt, cb)

// Forward JS exceptions to the tray's stderr log (no devtools in a packaged
// build). Diagnostic only — safe to keep.
const dbg = (msg) => { try { invoke('tray_debug', { msg: String(msg) }) } catch {} }
window.onerror = (m, src, line, col) => dbg(`popover onerror: ${m} @${line}:${col}`)
window.addEventListener('unhandledrejection', (e) => dbg(`popover rejection: ${e.reason}`))

// ── Reference data (mirrors the design's data.jsx, trimmed to what the tray draws) ──
const APPS = {
  'Antigravity':   { mono: 'Aᴳ', color: '#7C3AED' },
  'Google Chrome': { mono: 'Ch', color: '#3B82F6' },
  'Chrome':        { mono: 'Ch', color: '#3B82F6' },
  'Safari':        { mono: 'Sf', color: '#1B9DF0' },
  'Terminal':      { mono: '>_', color: '#111827' },
  'iTerm2':        { mono: '>_', color: '#111827' },
  'Warp':          { mono: '>_', color: '#01A4FF' },
  'GitHub':        { mono: 'Gh', color: '#181717' },
  'Claude':        { mono: 'Cl', color: '#D97757' },
  'Cursor':        { mono: 'Cu', color: '#111827' },
  'Code':          { mono: 'Co', color: '#3B82F6' },
  'Visual Studio Code': { mono: 'Co', color: '#3B82F6' },
  'Xcode':         { mono: 'Xc', color: '#1B7BE5' },
  'Slack':         { mono: 'Sl', color: '#E01E5A' },
  'Zoom':          { mono: 'Zm', color: '#2D8CFF' },
  'Linear':        { mono: 'Li', color: '#5E6AD2' },
  'Figma':         { mono: 'Fg', color: '#A259FF' },
  'Notion':        { mono: 'No', color: '#111111' },
  'Mail':          { mono: 'Ma', color: '#0EA5E9' },
}

const CAT_LABELS = {
  coding: 'Coding',
  code_review: 'Code review',
  meeting: 'Meeting',
  communication: 'Comms',
  design: 'Design',
  documentation: 'Docs',
  planning: 'Planning',
  deployment_devops: 'DevOps',
  research: 'Research',
  idle_personal: 'Idle',
}

// ── Elements ─────────────────────────────────────────────────────────────────
const $ = (id) => document.getElementById(id)
const brandDot = $('brand-dot')
const live = $('live')
const liveLabelText = $('live-label-text')
const liveMatch = $('live-match')
const appGlyph = $('app-glyph')
const liveCatDot = $('live-cat-dot')
const liveCatLabel = $('live-cat-label')
const liveTitle = $('live-title')
const timerEl = $('timer')
const liveSince = $('live-since')
const pauseSec = $('pause')
const pauseText = $('pause-text')
const pauseSub = $('pause-sub')
const pauseBtn = $('pause-btn')
const tileFocus = $('tile-focus')
const tileCoding = $('tile-coding')
const tileCodingSub = $('tile-coding-sub')
const tileReview = $('tile-review')
const tileComms = $('tile-comms')
const wlRule = $('wl-rule')
const wl = $('wl')
const wlBadge = $('wl-badge')
const wlText = $('wl-text')

// ── State for the local 1-second ticker ──────────────────────────────────────
let elapsed = 0       // live session seconds; re-synced on every status payload
let isTracking = false // healthy + has an active session
let tickId = null

// ── Formatters ───────────────────────────────────────────────────────────────
// "6h 30m" / "30m" / "0m" — zero-pads minutes only when hours are present.
function fmtTile(secs) {
  const m = Math.floor((secs || 0) / 60)
  const h = Math.floor(m / 60)
  const rm = m % 60
  if (h === 0) return `${rm}m`
  return `${h}h ${String(rm).padStart(2, '0')}m`
}

// Big timer split so seconds can render smaller: "2:05" + ":12".
function timerParts(secs) {
  const h = Math.floor(secs / 3600)
  const m = Math.floor((secs % 3600) / 60)
  const s = secs % 60
  return { hm: `${h}:${String(m).padStart(2, '0')}`, sec: `:${String(s).padStart(2, '0')}` }
}

function clockOf(date) {
  let h = date.getHours()
  const m = date.getMinutes()
  const period = h >= 12 ? 'PM' : 'AM'
  h = ((h + 11) % 12) + 1
  return `${h}:${String(m).padStart(2, '0')} ${period}`
}

function glyphFor(app) {
  return APPS[app] || { mono: (app || '?').slice(0, 2), color: '#6B6A67' }
}

// Several common dev apps (Terminal, Cursor, GitHub, Notion) ship near-black
// brand colors that vanish on the dark popover. Lift very dark glyph colors so
// the mark stays legible; light mode keeps the brand color untouched.
function glyphColor(hex) {
  const n = hex.replace('#', '')
  const r = parseInt(n.slice(0, 2), 16)
  const g = parseInt(n.slice(2, 4), 16)
  const b = parseInt(n.slice(4, 6), 16)
  const lum = (0.299 * r + 0.587 * g + 0.114 * b) / 255
  const dark = window.matchMedia && window.matchMedia('(prefers-color-scheme: dark)').matches
  if (!dark || lum >= 0.32) return hex
  const lift = (c) => Math.round(c + (255 - c) * 0.55)
  return '#' + [lift(r), lift(g), lift(b)].map((x) => x.toString(16).padStart(2, '0')).join('')
}

// ── Timer rendering ──────────────────────────────────────────────────────────
function paintTimer() {
  const { hm, sec } = timerParts(Math.max(0, elapsed))
  timerEl.innerHTML = `${hm}<span class="timer-sec">${sec}</span>`
  if (isTracking) {
    const started = new Date(Date.now() - elapsed * 1000)
    liveSince.textContent = `since ${clockOf(started)}`
  } else {
    liveSince.textContent = ''
  }
}

function startTicker() {
  if (tickId) return
  tickId = setInterval(() => {
    if (!isTracking) return
    elapsed += 1
    paintTimer()
  }, 1000)
}

// ── Render ───────────────────────────────────────────────────────────────────
function render(status) {
  const healthy = status.ui_reachable && status.healthy
  const hasActive = !!status.active_app

  // Brand dot reflects daemon health.
  brandDot.classList.toggle('down', !healthy)

  // ── Live block state ──────────────────────────────────────────────────────
  live.classList.remove('paused', 'idle')
  isTracking = healthy && hasActive

  if (!status.has_polled) {
    // First-tick hasn't completed yet — show a neutral connecting state so the
    // user doesn't see a misleading "PAUSED / Offline" during the 1–3 s startup
    // window before the poll loop delivers its first real status.
    live.classList.add('idle')
    liveLabelText.textContent = 'CONNECTING'
    liveMatch.textContent = ''
    elapsed = 0
  } else if (!healthy) {
    // Daemon is off / unreachable — tracking is effectively paused.
    live.classList.add('paused')
    liveLabelText.textContent = 'PAUSED'
    liveMatch.textContent = ''
    elapsed = 0
  } else if (hasActive) {
    liveLabelText.textContent = 'TRACKING NOW'
    const pct = Math.round((status.active_confidence || 0) * 100)
    liveMatch.textContent = pct > 0 ? `${pct}% match` : ''
    elapsed = status.active_elapsed_s || 0
  } else {
    live.classList.add('idle')
    liveLabelText.textContent = 'Nothing tracked right now'
    liveMatch.textContent = ''
    elapsed = 0
  }

  // App glyph + category + title — shown whenever we have an active app.
  if (hasActive) {
    const g = glyphFor(status.active_app)
    const gc = glyphColor(g.color)
    appGlyph.textContent = g.mono
    appGlyph.style.background = gc + '1A'
    appGlyph.style.color = gc

    const cat = status.active_category || 'idle_personal'
    liveCatDot.style.background = `var(--cat-${cat}, var(--ink-4))`
    liveCatLabel.textContent = CAT_LABELS[cat] || cat
    liveTitle.textContent = status.active_title || status.active_app
  } else {
    appGlyph.textContent = '··'
    appGlyph.style.background = 'var(--surface-2)'
    appGlyph.style.color = 'var(--ink-4)'
    liveCatDot.style.background = 'var(--ink-4)'
    liveCatLabel.textContent = !status.has_polled ? '' : healthy ? 'Idle' : 'Offline'
    liveTitle.textContent = !status.has_polled
      ? ''
      : healthy
        ? 'Meridian is watching — nothing to track yet.'
        : "Meridian's gone quiet."
  }
  paintTimer()

  // ── Pause / tracking toggle ───────────────────────────────────────────────
  if (healthy) {
    pauseSec.classList.remove('paused')
    pauseText.textContent = 'Pause tracking'
    pauseSub.textContent = ''
    pauseBtn.textContent = 'Pause'
  } else {
    pauseSec.classList.add('paused')
    pauseText.textContent = 'Tracking paused'
    pauseSub.textContent = "Meridian isn't recording"
    pauseBtn.textContent = 'Resume'
  }

  // ── Time tracker tiles ────────────────────────────────────────────────────
  tileFocus.textContent = fmtTile(status.focus_s)
  tileCoding.textContent = fmtTile(status.coding_s)
  tileReview.textContent = fmtTile(status.review_s)
  tileComms.textContent = fmtTile(status.comms_s)

  const auto = status.autonomous_s || 0
  if (auto >= 60) {
    tileCodingSub.textContent = `incl. ${fmtTile(auto)} autonomous AI`
    tileCodingSub.classList.add('accent')
  } else {
    tileCodingSub.textContent = 'focused work'
    tileCodingSub.classList.remove('accent')
  }

  // ── Worklogs awaiting (real drafts only) ──────────────────────────────────
  const drafts = status.drafts_count || 0
  if (drafts > 0) {
    wl.hidden = false
    wlRule.hidden = false
    wlBadge.textContent = String(drafts)
    wlText.textContent = drafts === 1 ? '1 worklog ready to approve' : 'Worklogs ready to approve'
  } else {
    wl.hidden = true
    wlRule.hidden = true
  }
}

// ── Events + actions ─────────────────────────────────────────────────────────
listen('status-update', (event) => { render(event.payload); resizeToContent() })

$('btn-open-head').addEventListener('click', () => invoke('open_dashboard'))
$('btn-open').addEventListener('click', () => invoke('open_dashboard'))
$('btn-perms-head').addEventListener('click', () =>
  invoke('open_permission_pane', { pane: 'screen_recording' }).catch(console.error))
$('btn-perms').addEventListener('click', () =>
  invoke('open_permission_pane', { pane: 'screen_recording' }).catch(console.error))
$('btn-quit').addEventListener('click', () => invoke('quit_app').catch(console.error))
wl.addEventListener('click', () => invoke('open_worklogs'))

pauseBtn.addEventListener('click', () => {
  // Healthy → tell the daemon it's running so it stops (Pause).
  // Unhealthy → tell it it's stopped so it starts (Resume).
  const running = !brandDot.classList.contains('down')
  invoke('toggle_daemon', { is_running: running }).catch(console.error)
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
    getCurrentWindow().setSize(new LogicalSize(384, h))
      .then(() => dbg(`popover resize -> measured pop height=${h}`))
      .catch((e) => dbg(`popover setSize FAILED: ${e}`))
  } catch (e) { dbg(`popover resize threw: ${e}`) }
}

// Re-fit on ANY card height change — web-font swap (Instrument Serif / mono load
// after first paint and change metrics), the worklog row showing/hiding, etc.
// Without this the window keeps its first (pre-font) height and the taller card
// overflows → clipped rounded bottom + scrollbar.
if (window.ResizeObserver) {
  new ResizeObserver(() => resizeToContent()).observe(popEl)
}
if (document.fonts && document.fonts.ready) {
  document.fonts.ready.then(() => resizeToContent()).catch(() => {})
}

// ── Boot ──────────────────────────────────────────────────────────────────────
startTicker()
invoke('get_status').then((s) => { render(s); resizeToContent() }).catch(() => {})
