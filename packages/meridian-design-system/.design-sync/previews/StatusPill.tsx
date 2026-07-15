//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { StatusPill } from 'meridian-design-system'

export function Drafted() {
  return <StatusPill status="drafted" />
}

export function Approved() {
  // Real callers pass the server-computed `is_terminal` explicitly (the
  // keyword-guess fallback below is for callers with only a raw string,
  // e.g. a command bar) — this is the actual "done" state most rows show.
  return <StatusPill status="approved" isTerminal />
}

export function Failed() {
  return <StatusPill status="failed" isTerminal />
}

export function KeywordFallback() {
  // No isTerminal passed — falls back to a keyword guess on the raw string.
  return <StatusPill status="shipped" />
}

export function Stack() {
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
      <StatusPill status="drafted" isTerminal={false} />
      <StatusPill status="proposed" isTerminal={false} />
      <StatusPill status="approved" isTerminal />
      <StatusPill status="posted" isTerminal />
      <StatusPill status="dismissed" isTerminal />
    </div>
  )
}
