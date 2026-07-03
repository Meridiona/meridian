//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// A small fixed pill that floats above the timeline once there are drafts to
// review (connected mode only). Clicking opens the Review modal.

'use client'

export function FloatingDraftsPill({ count, onClick }: { count: number; onClick: () => void }) {
  if (count <= 0) return null
  return (
    <button onClick={onClick}
      className="absolute left-1/2 -translate-x-1/2 bottom-6 z-30 inline-flex items-center gap-2.5 rounded-full pl-4 pr-2.5 py-2.5 transition-transform active:scale-95"
      style={{ background: 'var(--color-state-pending)', color: '#fff', boxShadow: '0 12px 28px -6px rgba(245,158,11,0.55)' }}>
      <span className="text-[14px] leading-none">↑</span>
      <span className="mt-body-sm" style={{ fontWeight: 700 }}>Review drafts</span>
      <span className="mt-mono-sm inline-flex items-center justify-center rounded-full"
        style={{ minWidth: 22, height: 22, padding: '0 6px', background: 'rgba(0,0,0,0.24)', color: '#fff', fontWeight: 800 }}>
        {count}
      </span>
    </button>
  )
}
