//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
/* global __TAURI__ */
'use strict'

// withGlobalTauri=true exposes the bridge; the wizard drives everything through
// Rust commands (open_permission_pane, …) — no Node server, no fetch.
const invoke = (cmd, args) => __TAURI__.core.invoke(cmd, args)

const STEP_COUNT = 5
let step = 0

const railEl = document.getElementById('rail')
const backBtn = document.getElementById('back')
const nextBtn = document.getElementById('next')
const errEl = document.getElementById('err')

// ── Step rail ─────────────────────────────────────────────────────────────────
for (let i = 0; i < STEP_COUNT; i++) {
  const pip = document.createElement('div')
  pip.className = 'pip'
  railEl.appendChild(pip)
}
const pips = [...railEl.children]

function render() {
  document.querySelectorAll('.step').forEach((el) => {
    el.classList.toggle('active', Number(el.dataset.step) === step)
  })
  pips.forEach((pip, i) => {
    pip.classList.toggle('done', i < step)
    pip.classList.toggle('active', i === step)
  })
  backBtn.style.visibility = step === 0 ? 'hidden' : 'visible'
  nextBtn.textContent = step === STEP_COUNT - 1 ? 'Finish' : 'Continue'
  errEl.textContent = ''
}

backBtn.addEventListener('click', () => {
  if (step > 0) { step -= 1; render() }
})

nextBtn.addEventListener('click', () => {
  if (step < STEP_COUNT - 1) {
    step += 1
    render()
  } else {
    // Finish — close the wizard window. Onboarding-complete persistence is
    // wired in a later slice.
    __TAURI__.window.getCurrentWindow().close()
  }
})

// ── Permission deep-links (real now) ──────────────────────────────────────────
// Live grant detection is functional (data-flow based) and lands in the next
// slice; the deep-link buttons work today regardless of detection.
document.querySelectorAll('button[data-pane]').forEach((btn) => {
  btn.addEventListener('click', () => {
    invoke('open_permission_pane', { pane: btn.dataset.pane }).catch((e) => {
      errEl.textContent = String(e)
    })
  })
})

// ── Tracker auth (wired in a later slice) ─────────────────────────────────────
// Disabled until the oauth-login integration shape is confirmed, rather than
// faking a flow that isn't there.
document.querySelectorAll('button[data-provider]').forEach((btn) => {
  btn.disabled = true
  btn.title = 'Tracker connection is wired in the next step'
})

render()
