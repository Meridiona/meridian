//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { SegBar } from 'meridian-design-system'

function Labeled({ label, segments }: { label: string; segments: Array<{ cat?: string; value: number; color?: string }> }) {
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 4, width: 240 }}>
      <span style={{ fontSize: 11 }}>{label}</span>
      <SegBar segments={segments} height={6} />
    </div>
  )
}

export function ByCategory() {
  return (
    <Labeled
      label="Today's time by category"
      segments={[
        { cat: 'coding', value: 240 },
        { cat: 'meeting', value: 60 },
        { cat: 'communication', value: 30 },
        { cat: 'idle_personal', value: 15 },
      ]}
    />
  )
}

export function CustomColors() {
  return (
    <Labeled
      label="Approval breakdown"
      segments={[
        { value: 12, color: '#10B981' },
        { value: 3, color: '#F59E0B' },
        { value: 1, color: '#C7C2D6' },
      ]}
    />
  )
}
