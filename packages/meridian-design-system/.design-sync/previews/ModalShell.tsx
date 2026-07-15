//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { ModalShell } from 'meridian-design-system'

// ModalShell's own markup is `fixed inset-0` (a real full-viewport backdrop).
// `transform` on this wrapper makes it a CSS containing block for fixed
// descendants, so the modal fills THIS box instead of escaping to the true
// page viewport (which the capture harness measures as ~0 height here) —
// composition-only fix, ModalShell's source is untouched.
function Frame({ children }: { children: React.ReactNode }) {
  return (
    <div style={{ position: 'relative', width: 520, height: 420, transform: 'scale(1)', overflow: 'hidden' }}>
      {children}
    </div>
  )
}

export function Default() {
  return (
    <Frame>
      <ModalShell title="Review worklog" onClose={() => {}} maxWidth={480}>
        <div style={{ padding: 20 }}>
          <p style={{ fontSize: 13, margin: 0 }}>MER-482 · Fix ETL gap detection</p>
          <p style={{ fontSize: 12, marginTop: 8, color: '#888' }}>
            Closed a gap-detection bug where a sleep spanning an ETL run boundary was silently dropped.
          </p>
        </div>
      </ModalShell>
    </Frame>
  )
}

export function ScrollInside() {
  return (
    <Frame>
      <ModalShell title="Settings" onClose={() => {}} maxWidth={520} scrollInside>
        <div style={{ padding: 20 }}>
          {Array.from({ length: 6 }).map((_, i) => (
            <p key={i} style={{ fontSize: 12, margin: '8px 0' }}>Setting row {i + 1}</p>
          ))}
        </div>
      </ModalShell>
    </Frame>
  )
}
