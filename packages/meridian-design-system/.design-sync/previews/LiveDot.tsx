//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { LiveDot } from 'meridian-design-system'

export function Default() {
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
      <LiveDot size={8} />
      <span style={{ fontSize: 12 }}>Tracking now</span>
    </div>
  )
}

export function Large() {
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
      <LiveDot size={14} />
      <span style={{ fontSize: 12 }}>Recording session</span>
    </div>
  )
}
