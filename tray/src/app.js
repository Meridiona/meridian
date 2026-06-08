// meridian — normalises screenpipe activity into structured app sessions
/* global __TAURI__ */
'use strict'

const invoke = (cmd, args) => __TAURI__.core.invoke(cmd, args)
const listen  = (evt, cb)  => __TAURI__.event.listen(evt, cb)

// ── State ────────────────────────────────────────────────────────────────────
let elapsed = 0   // local 1-second counter synced on each status-update
let elapsedTimerId = null

// ── Elements ────────────────────────────────────────────────────────────────
const dot          = document.getElementById('status-dot')
const sessionApp   = document.getElementById('session-app')
const sessionElapsed = document.getElementById('session-elapsed')
const statFocus    = document.getElementById('stat-focus')
const statSwitches = document.getElementById('stat-switches')
const worklogsSection = document.getElementById('worklogs-section')
const worklogsRule    = document.getElementById('worklogs-rule')
const worklogsLabel   = document.getElementById('worklogs-label')
const btnOpen      = document.getElementById('btn-open')
const btnWorklogs  = document.getElementById('btn-worklogs')
const worklogsBtn  = document.getElementById('worklogs-btn')

// ── Formatters ───────────────────────────────────────────────────────────────
function fmtDuration(secs) {
  if (secs < 60) return 'Just getting started'
  const m = Math.floor(secs / 60)
  const h = Math.floor(m / 60)
  const rm = m % 60
  if (h === 0) return m === 1 ? '1 minute' : `${m} minutes`
  if (rm === 0) return h === 1 ? '1 hour' : `${h} hours`
  const hStr = h === 1 ? '1 hour' : `${h} hours`
  const mStr = rm === 1 ? '1 minute' : `${rm} minutes`
  return `${hStr} ${mStr}`
}

function fmtElapsed(secs) {
  if (secs < 60) return 'moments ago'
  return 'for ' + fmtDuration(secs)
}

function describeApp(appName, elapsedSecs) {
  const lower = appName.toLowerCase()
  const timeStr = fmtElapsed(elapsedSecs)
  if (/claude|cursor|codex/i.test(lower)) return `In a session with ${appName} ${timeStr}`
  if (/code|xcode|vim|neovim|emacs/i.test(lower)) return `Deep in ${appName} ${timeStr}`
  if (/terminal|iterm|warp|kitty/i.test(lower)) return `In ${appName} — in the zone ${timeStr}`
  if (/slack|teams|discord/i.test(lower)) return `In ${appName} ${timeStr}`
  if (/figma|sketch/i.test(lower)) return `Designing in ${appName} ${timeStr}`
  if (/notion|obsidian|notes/i.test(lower)) return `Writing in ${appName} ${timeStr}`
  return `In ${appName} ${timeStr}`
}

// focusDesc is Rust's pre-formatted duration string (e.g. "6 hours 12 minutes").
// We still apply the < 30 min threshold here for the "of real work today" copy.
function fmtFocus(secs, focusDesc) {
  if (secs < 1800) return 'Just getting started'  // < 30 min
  return (focusDesc || fmtDuration(secs)) + ' of real work today'
}

function fmtSwitches(count) {
  if (count === 0) return 'No context switches — focused day'
  if (count === 1) return '1 context switch'
  return `${count} context switches`
}

// ── Render ───────────────────────────────────────────────────────────────────
function render(status) {
  // Status dot
  dot.className = 'status-dot'
  if (!status.ui_reachable) {
    dot.classList.add('unhealthy')
  } else if (!status.healthy) {
    dot.classList.add('unhealthy')
  } else if (status.drafts_count > 0) {
    dot.classList.add('pending')
  } else {
    dot.classList.add('healthy')
  }

  // Active session
  clearShimmer(sessionApp)
  clearShimmer(statFocus)
  clearShimmer(statSwitches)

  if (status.active_app) {
    elapsed = status.active_elapsed_s || 0
    // Use Rust pre-formatted string on poll; 1s timer re-formats locally.
    sessionApp.textContent = status.active_desc || describeApp(status.active_app, elapsed)
    sessionElapsed.textContent = ''
    startElapsedTimer(status.active_app)
  } else {
    elapsed = 0
    stopElapsedTimer()
    sessionApp.textContent = status.healthy
      ? 'Nothing tracked right now'
      : 'Meridian\'s gone quiet.'
    sessionElapsed.textContent = ''
  }

  // Stats — use Rust focus_desc, but apply the ≥30 min threshold for "real work" copy.
  statFocus.textContent = fmtFocus(status.focus_s || 0, status.focus_desc)
  statSwitches.textContent = fmtSwitches(status.switch_count || 0)

  // Worklogs
  const drafts = status.drafts_count || 0
  if (drafts > 0) {
    worklogsSection.style.display = ''
    worklogsRule.style.display = ''
    worklogsLabel.textContent = drafts === 1
      ? '1 draft waiting on you'
      : `${drafts} drafts waiting on you`
  } else {
    worklogsSection.style.display = 'none'
    worklogsRule.style.display = 'none'
  }
}

// ── Elapsed timer (local 1s tick) ─────────────────────────────────────────────
let _activeApp = null

function startElapsedTimer(appName) {
  _activeApp = appName
  if (elapsedTimerId) return   // already running
  elapsedTimerId = setInterval(() => {
    elapsed += 1
    if (_activeApp) {
      sessionApp.textContent = describeApp(_activeApp, elapsed)
    }
  }, 1000)
}

function stopElapsedTimer() {
  _activeApp = null
  if (elapsedTimerId) {
    clearInterval(elapsedTimerId)
    elapsedTimerId = null
  }
}

// ── Shimmer helpers ──────────────────────────────────────────────────────────
function clearShimmer(el) {
  const s = el.querySelector('.shimmer')
  if (s) s.remove()
}

// ── Events ───────────────────────────────────────────────────────────────────
listen('status-update', (event) => {
  render(event.payload)
})

btnOpen.addEventListener('click', () => invoke('open_dashboard'))
btnWorklogs.addEventListener('click', () => invoke('open_worklogs'))
worklogsBtn.addEventListener('click', () => invoke('open_worklogs'))

// ── Boot ──────────────────────────────────────────────────────────────────────
invoke('get_status').then(render).catch(() => {})
