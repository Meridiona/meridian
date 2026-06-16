//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
/* global __TAURI__ */
'use strict'

// withGlobalTauri=true exposes the bridge; these call the RUST commands directly
// (get_today / get_week over meridian-core), no fetch, no Node server.
const invoke = (cmd, args) => __TAURI__.core.invoke(cmd, args)

const hrs = (secs) => (secs / 3600).toFixed(1) + 'h'

async function loadToday() {
  const el = document.getElementById('today')
  try {
    const t = await invoke('get_today')
    document.getElementById('date').textContent = t.date
    const row = (label, value) => `<li><span>${label}</span><b>${value}</b></li>`
    el.innerHTML =
      row('Focus', `${hrs(t.focus_s)} · ${t.sessions.length} sessions`) +
      row('Context switches', t.switch_count) +
      row('Agent', `${hrs(t.agent_s)} (autonomous ${hrs(t.autonomous_s)})`) +
      row('Engaged', `${hrs(t.engaged_s)} · idle ${hrs(t.idle_s)}`) +
      row('Tasks touched', Object.keys(t.task_totals).length)
  } catch (e) {
    el.innerHTML = `<li class="err">Error: ${e}</li>`
  }
}

async function loadWeek() {
  const el = document.getElementById('week')
  try {
    const w = await invoke('get_week')
    el.innerHTML =
      w.days
        .map(
          (d) =>
            `<div class="day"><span class="${d.isToday ? 'today' : ''}">${d.day} ${d.date}${d.isToday ? ' ←' : ''}</span><span>${hrs(d.total_s)}</span></div>`,
        )
        .join('') + `<div class="total">Week total: ${hrs(w.total_s)}</div>`
  } catch (e) {
    el.innerHTML = `<span class="err">Error: ${e}</span>`
  }
}

loadToday()
loadWeek()
