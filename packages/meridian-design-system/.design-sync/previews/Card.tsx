//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { Card } from 'meridian-design-system'

export function Default() {
  return (
    <Card style={{ padding: 16, maxWidth: 280 }}>
      <p style={{ fontSize: 13, margin: 0 }}>MER-482 · Fix ETL gap detection</p>
      <p style={{ fontSize: 11, margin: '4px 0 0', color: '#888' }}>2h 15m tracked today</p>
    </Card>
  )
}

export function AsSection() {
  return (
    <Card as="section" className="rise" style={{ padding: 20, maxWidth: 320 }}>
      <p style={{ fontSize: 14, fontWeight: 600, margin: 0 }}>Today's focus</p>
      <p style={{ fontSize: 12, margin: '6px 0 0' }}>4 sessions matched, 1 proposal awaiting review.</p>
    </Card>
  )
}
