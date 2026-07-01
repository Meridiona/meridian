//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// A small fixed pill that floats above the timeline once there are drafts to
// review (connected mode only). Clicking opens the Review modal.

'use client'

export function FloatingDraftsPill({ count, onClick }: { count: number; onClick: () => void }) {
  if (count <= 0) return null
  return (
    <button onClick={onClick}
      className="absolute left-1/2 -translate-x-1/2 bottom-6 z-30 inline-flex items-center gap-2 rounded-full px-4 py-2.5 transition-transform active:scale-95"
      style={{ background: 'var(--chip)', color: '#fff', boxShadow: '0 12px 30px -8px rgba(20,16,40,0.45)' }}>
      <span className="text-[14px] leading-none">↑</span>
      <span className="mt-body-sm" style={{ fontWeight: 700 }}>{count} to review</span>
    </button>
  )
}
