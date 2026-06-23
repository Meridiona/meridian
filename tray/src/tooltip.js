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

const STATUS_LABELS = {
  todo:        'To do',
  in_progress: 'In progress',
  in_review:   'In review',
  done:        'Done',
  cancelled:   'Cancelled',
}

function fmtDur(s) {
  if (!s) return '0m'
  const m = Math.floor(s / 60)
  const h = Math.floor(m / 60)
  const rm = m % 60
  if (h === 0) return `${rm}m`
  return rm ? `${h}h ${rm}m` : `${h}h`
}

function priClass(p) {
  if (!p) return ''
  const l = p.toLowerCase()
  if (l === 'high' || l === 'urgent' || l === 'critical') return 'high'
  if (l === 'medium' || l === 'normal') return 'medium'
  return 'low'
}

function render(status) {
  const hasTask = !!status.task_key

  if (!hasTask) {
    card.classList.add('empty')
    ttKey.textContent = '—'
    ttStatus.textContent = ''
    ttPri.textContent = ''
    ttTitle.textContent = 'No task tracked today'
    ttCtx.textContent = ''
    return
  }

  card.classList.remove('empty')

  ttKey.textContent = status.task_key
  ttStatus.textContent = STATUS_LABELS[status.task_status_category] || (status.task_status_category || '')
  const priLabel = status.task_priority || ''
  ttPri.textContent = priLabel ? `• ${priLabel}` : ''
  ttPri.className = `tt-priority ${priClass(priLabel)}`
  ttTitle.textContent = status.task_title || status.task_key

  // Progress
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

  // Context: active window title or fallback
  ttCtx.textContent = status.active_title || ''

  // Resize window to card height.
  const h = Math.ceil(card.getBoundingClientRect().height)
  if (h > 50) {
    try {
      const { LogicalSize, getCurrentWindow } = window.__TAURI__.window
      getCurrentWindow().setSize(new LogicalSize(288, h)).catch(() => {})
    } catch {}
  }
}

listen('status-update', (e) => render(e.payload))
invoke('get_status').then(render).catch(() => {})
