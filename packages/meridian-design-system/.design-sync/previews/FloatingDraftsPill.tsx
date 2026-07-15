//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { FloatingDraftsPill } from 'meridian-design-system'

function Frame({ children }: { children: React.ReactNode }) {
  return <div style={{ position: 'relative', width: 420, height: 90 }}>{children}</div>
}

export function FewDrafts() {
  return <Frame><FloatingDraftsPill count={3} onClick={() => {}} /></Frame>
}

export function ManyDrafts() {
  return <Frame><FloatingDraftsPill count={12} onClick={() => {}} /></Frame>
}
