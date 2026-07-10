//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { CurvedArrow } from 'meridian-design-system'

export function AboveCTA() {
  return (
    <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'flex-end', gap: 2 }}>
      <CurvedArrow size={40} />
      <button style={{ fontSize: 12, padding: '6px 12px', borderRadius: 8, background: 'var(--color-state-proposal)', color: '#fff', border: 'none' }}>Connect a tracker</button>
    </div>
  )
}

export function Small() {
  return <CurvedArrow size={24} />
}

export function Large() {
  return <CurvedArrow size={56} />
}
