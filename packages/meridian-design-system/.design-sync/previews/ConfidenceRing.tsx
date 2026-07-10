//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { ConfidenceRing } from 'meridian-design-system'

function Row({ label, value }: { label: string; value: number }) {
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
      <ConfidenceRing value={value} size={16} />
      <span style={{ fontSize: 12 }}>{label} — {Math.round(value * 100)}% match confidence</span>
    </div>
  )
}

export function High() {
  return <Row label="MER-482 · Fix ETL gap detection" value={0.92} />
}

export function Medium() {
  return <Row label="MER-501 · Session linking" value={0.65} />
}

export function Low() {
  return <Row label="Untracked candidate" value={0.3} />
}

export function Stack() {
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
      <Row label="High confidence" value={0.95} />
      <Row label="Medium confidence" value={0.7} />
      <Row label="Low confidence" value={0.35} />
    </div>
  )
}
