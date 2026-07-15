//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { TimeByApp } from 'meridian-design-system'

function session(app: string, cat: string, dur: number, id: number) {
  return {
    id, app, started_at: '2026-07-09T14:00:00Z', dur, cat, titles: [app],
    explain: null, routing: null, session_type: null, task_key: null, candidates: [],
    confidence: 0.8, method: 'mlx', link_method: null, link_confidence: null, summary: null,
  }
}

export function Default() {
  const sessions = [
    session('Visual Studio Code', 'coding', 9000, 1),
    session('Google Chrome', 'research', 3600, 2),
    session('Slack', 'communication', 1800, 3),
    session('Figma', 'design', 1200, 4),
    session('Terminal', 'coding', 900, 5),
  ]
  return <div style={{ width: 320 }}><TimeByApp sessions={sessions} /></div>
}

export function WithCodingAgents() {
  const sessions = [session('Visual Studio Code', 'coding', 5400, 1), session('Google Chrome', 'research', 1800, 2)]
  const agentTotals = [{ app: 'Claude Code', total_s: 7200 }, { app: 'Codex', total_s: 1800 }]
  return <div style={{ width: 320 }}><TimeByApp sessions={sessions} agentTotals={agentTotals} /></div>
}

export function Empty() {
  return <div style={{ width: 320 }}><TimeByApp sessions={[]} /></div>
}
