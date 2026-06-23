//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
/* global __TAURI__ */
'use strict'

const listen = (evt, cb) => __TAURI__.event.listen(evt, cb)
const invoke = (cmd, args) => __TAURI__.core.invoke(cmd, args)

const card     = document.getElementById('tt-card')
const ttKey    = document.getElementById('tt-key')
const ttStatus = document.getElementById('tt-status')
const ttPri    = document.getElementById('tt-priority')
const ttTitle  = document.getElementById('tt-title')
const ttTime   = document.getElementById('tt-time')
const ttPct    = document.getElementById('tt-pct')
const ttFill   = document.getElementById('tt-bar-fill')
const ttCtx    = document.getElementById('tt-ctx')

// Window is 300px wide; body has 10px top + 24px bottom transparent padding for
// the card's drop-shadow (mirrors tooltip.css). Resize must add that back.
const WIN_W = 300
const PAD_V = 10 + 24

const STATUS_LABELS = {
  todo:        'To do',
  in_progress: 'In progress',
  in_review:   'In review',
  done:        'Done',
  cancelled:   'Cancelled',
}

const CAT_LABELS = {
  coding:        'Coding',
  code_review:   'Code review',
  communication: 'Comms',
  research:      'Research',
  meeting:       'Meeting',
  design:        'Design',
  writing:       'Writing',
  learning:      'Learning',
}

function fmtDur(s) {
  if (!s) return '0m'
  const m = Math.floor(s / 60)
  const h = Math.floor(m / 60)
  const rm = m % 60
  if (h === 0) return `${rm}m`
  return rm ? `${h}h ${rm}m` : `${h}h`
}

function prettyCat(c) {
  if (!c) return 'Live'
  return CAT_LABELS[c] || (c[0].toUpperCase() + c.slice(1).replace(/_/g, ' '))
}

function priClass(p) {
  if (!p) return ''
  const l = p.toLowerCase()
  if (l === 'high' || l === 'urgent' || l === 'critical') return 'high'
  if (l === 'medium' || l === 'normal') return 'medium'
  return 'low'
}

function render(status) {
  // ── State 1: a classified task today → the full task card. ──
  if (status.task_key) {
    card.classList.remove('empty', 'live')
    renderTask(status)
    resize()
    return
  }

  // ── State 2: no task, but a live session → show what you're doing now. ──
  if (status.has_polled && status.healthy && status.active_app) {
    card.classList.remove('empty')
    card.classList.add('live')
    ttKey.textContent   = prettyCat(status.active_category)
    ttStatus.textContent = ''
    ttPri.textContent   = ''
    ttPri.className     = 'tt-priority'
    ttTitle.textContent = status.active_desc || `Working in ${status.active_app}`
    ttCtx.textContent   = status.active_title || status.active_app || ''
    resize()
    return
  }

  // ── State 3: nothing to track (idle / connecting / offline). ──
  card.classList.remove('live')
  card.classList.add('empty')
  ttKey.textContent    = '—'
  ttStatus.textContent = ''
  ttPri.textContent    = ''
  ttPri.className      = 'tt-priority'
  ttTitle.textContent  = !status.has_polled ? 'Connecting…'
    : !status.healthy  ? "Meridian's paused"
    : 'Watching — nothing to track yet'
  ttCtx.textContent    = ''
  resize()
}

function renderTask(status) {
  ttKey.textContent = status.task_key
  ttStatus.textContent = STATUS_LABELS[status.task_status_category] || (status.task_status_category || '')
  const priLabel = status.task_priority || ''
  ttPri.textContent = priLabel ? `• ${priLabel}` : ''
  ttPri.className = `tt-priority ${priClass(priLabel)}`
  ttTitle.textContent = status.task_title || status.task_key

  const spent = status.task_spent_today_s || 0
  const est   = status.task_estimate_s || 0
  const pct   = status.task_percent != null
    ? Math.round(status.task_percent * 100)
    : est > 0 ? Math.min(100, Math.round(spent / est * 100)) : null

  if (pct !== null && est > 0) {
    ttTime.textContent = `${fmtDur(spent)} of ${fmtDur(est)} est`
    ttPct.textContent  = `${pct}%`
    ttFill.style.width = `${Math.min(pct, 100)}%`
  } else if (spent > 0) {
    ttTime.textContent = `${fmtDur(spent)} today`
    ttPct.textContent  = ''
    ttFill.style.width = '0%'
  } else {
    ttTime.textContent = 'No time logged today'
    ttPct.textContent  = ''
    ttFill.style.width = '0%'
  }

  ttCtx.textContent = status.active_title || ''
}

// Size the window to the card's height plus the transparent body padding, so
// the card never overflows and the shadow is never clipped into a rectangle.
function resize() {
  const h = Math.ceil(card.getBoundingClientRect().height) + PAD_V
  if (h < 50) return
  try {
    const { LogicalSize, getCurrentWindow } = window.__TAURI__.window
    getCurrentWindow().setSize(new LogicalSize(WIN_W, h)).catch(() => {})
  } catch {}
}

listen('status-update', (e) => render(e.payload))
invoke('get_status').then(render).catch(() => {})
