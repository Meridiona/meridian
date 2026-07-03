//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Shared field primitives for every Settings section — the themed replacement
// for the old SettingsView's private SectionCard/SectionHeader/FieldRow/
// SaveButton helpers, which were hardcoded to the legacy --surface/--ink/
// --accent tokens. These use the same --t-*/--color-state-* tokens as the
// rest of the Timeline app (ModalShell, Toolbar, TimelineColumn), so Settings
// now shares one live theme with everything else — switching lilac/blush/ink
// anywhere updates Settings too, with no separate palette to fall out of sync.

'use client'

export type SaveStatus = 'idle' | 'saved' | 'error'

export function SectionCard({ children }: { children: React.ReactNode }) {
  return (
    <div className="rounded-2xl p-5 flex flex-col gap-4 bg-card"
      style={{ border: '1px solid var(--t-card-border)' }}>
      {children}
    </div>
  )
}

export function SectionHeader({ children }: { children: React.ReactNode }) {
  return <p className="mt-label" style={{ color: 'var(--t-faint)' }}>{children}</p>
}

export function FieldRow({ label, description, children }: {
  label: string
  description?: string
  children: React.ReactNode
}) {
  return (
    <div className="flex items-center justify-between gap-6">
      <div className="min-w-0">
        <p className="mt-body-sm font-medium" style={{ color: 'var(--t-title)' }}>{label}</p>
        {description && (
          <p className="text-[11px] mt-0.5" style={{ color: 'var(--t-faint)' }}>{description}</p>
        )}
      </div>
      <div className="shrink-0 flex items-center gap-2">{children}</div>
    </div>
  )
}

/** Small themed action button — the same visual weight as SaveButton's own
 *  button but usable standalone (e.g. "Go to Setup", "Open OpenObserve"). */
export function SettingsButton({ onClick, children, disabled, variant = 'solid' }: {
  onClick: () => void
  children: React.ReactNode
  disabled?: boolean
  variant?: 'solid' | 'outline'
}) {
  const solid = variant === 'solid'
  return (
    <button type="button" onClick={onClick} disabled={disabled}
      className="mt-body-sm font-semibold rounded-lg px-3.5 py-1.5"
      style={{
        background: solid ? 'var(--color-state-proposal)' : 'transparent',
        color: solid ? '#fff' : 'var(--color-state-proposal)',
        border: solid ? 'none' : '1px solid var(--color-state-proposal)',
        opacity: disabled ? 0.65 : 1,
        cursor: disabled ? 'not-allowed' : 'pointer',
      }}>
      {children}
    </button>
  )
}

export function SaveButton({ onClick, status }: { onClick: () => void; status: SaveStatus }) {
  return (
    <div className="flex items-center gap-2.5 pt-2" style={{ borderTop: '1px solid var(--t-hair)' }}>
      <SettingsButton onClick={onClick}>Save</SettingsButton>
      {status === 'saved' && <span className="text-[12px]" style={{ color: 'var(--color-state-approved)' }}>Saved</span>}
      {status === 'error' && <span className="text-[12px]" style={{ color: 'var(--color-state-pending)' }}>Failed to save</span>}
    </div>
  )
}
