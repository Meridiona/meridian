//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { TimeByCategory } from 'meridian-design-system'

function session(cat: string, dur: number, id: number) {
  return {
    id, app: 'App', started_at: '2026-07-09T14:00:00Z', dur, cat, titles: [],
    explain: null, routing: null, session_type: null, task_key: null, candidates: [],
    confidence: 0.8, method: 'mlx', link_method: null, link_confidence: null, summary: null,
  }
}

export function Default() {
  const sessions = [
    session('coding', 14400, 1),
    session('meeting', 3600, 2),
    session('communication', 1800, 3),
    session('research', 1200, 4),
    session('planning', 900, 5),
  ]
  return <div style={{ width: 320 }}><TimeByCategory sessions={sessions} /></div>
}

export function WithAgentSeconds() {
  const sessions = [session('coding', 3600, 1), session('meeting', 1800, 2)]
  return <div style={{ width: 320 }}><TimeByCategory sessions={sessions} agentSeconds={7200} /></div>
}

export function Empty() {
  return <div style={{ width: 320 }}><TimeByCategory sessions={[]} /></div>
}
