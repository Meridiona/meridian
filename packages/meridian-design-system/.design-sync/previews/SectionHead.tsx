//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { SectionHead } from 'meridian-design-system'

export function Default() {
  return <SectionHead kicker="TODAY'S FOCUS" title="Active tasks" />
}

export function WithRight() {
  return (
    <SectionHead
      kicker="TIME BY APP"
      title="Where your day went"
      right={<span style={{ fontSize: 11, color: '#888' }}>6h 42m total</span>}
    />
  )
}
