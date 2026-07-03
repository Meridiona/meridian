//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// "Report" / get-in-touch modal, ported from the Meridian Timeline design
// mock (Claude Design project b8656e29-ae04-4f69-b17f-d5fab4d00f3a, the
// REPORT / GET IN TOUCH MODAL block) opened from the toolbar nav pill's
// "Report" item (see Toolbar.tsx's MeridianNavPill). The gradient hero header
// is a fixed brand treatment — deliberately independent of the light/blush/ink
// surface theme, same rule the nav pill's dark lockup follows — everything
// below it (channel cards, buttons, footer) uses the live `--t-*` tokens so it
// matches whichever theme is active. Overlay/backdrop/Escape-to-close mirrors
// ModalShell; the hero header replaces ModalShell's plain title bar so this
// doesn't wrap it.

'use client'

import { useEffect } from 'react'

const CHANNELS: {
  name: string
  handle: string
  href: string
  icon: React.ReactNode
}[] = [
  {
    name: 'Email',
    handle: 'hey@meridiona.com',
    href: 'mailto:hey@meridiona.com',
    icon: (
      <svg width="17" height="17" viewBox="0 0 24 24" fill="none">
        <rect x="2.5" y="5" width="19" height="14" rx="3" stroke="#EA4335" strokeWidth="1.8" />
        <path d="M3.5 6.5 L12 12.5 L20.5 6.5" stroke="#EA4335" strokeWidth="1.8" strokeLinecap="round" />
      </svg>
    ),
  },
  {
    name: 'Discord',
    handle: 'Join the community',
    href: 'https://discord.gg/meridiona',
    icon: (
      <svg width="18" height="18" viewBox="0 0 24 24" fill="#5865F2">
        <path d="M19.5 5.6A16 16 0 0 0 15.5 4.4l-.2.4a12 12 0 0 1 3.4 1.7 11 11 0 0 0-9.4 0 12 12 0 0 1 3.4-1.7l-.2-.4A16 16 0 0 0 4.5 5.6 16.5 16.5 0 0 0 1.7 17a16 16 0 0 0 4.9 2.5l.6-1a10.5 10.5 0 0 1-1.7-.8l.4-.3a11.5 11.5 0 0 0 10.2 0l.4.3a10.5 10.5 0 0 1-1.7.8l.6 1A16 16 0 0 0 22.3 17 16.5 16.5 0 0 0 19.5 5.6ZM8.6 14.4c-.9 0-1.7-.9-1.7-1.9s.8-1.9 1.7-1.9 1.7.9 1.7 1.9-.8 1.9-1.7 1.9Zm6.8 0c-.9 0-1.7-.9-1.7-1.9s.8-1.9 1.7-1.9 1.7.9 1.7 1.9-.8 1.9-1.7 1.9Z" />
      </svg>
    ),
  },
  {
    name: 'GitHub',
    handle: 'Issues & releases',
    href: 'https://github.com/meridiona',
    icon: (
      <svg width="18" height="18" viewBox="0 0 24 24" style={{ fill: 'var(--t-title)' }}>
        <path d="M12 2A10 10 0 0 0 8.8 21.5c.5.1.7-.2.7-.5v-1.7c-2.8.6-3.4-1.3-3.4-1.3-.5-1.2-1.1-1.5-1.1-1.5-.9-.6.1-.6.1-.6 1 .1 1.5 1 1.5 1 .9 1.5 2.3 1.1 2.9.8.1-.6.3-1.1.6-1.3-2.2-.3-4.6-1.1-4.6-4.9 0-1.1.4-2 1-2.7-.1-.3-.4-1.3.1-2.6 0 0 .8-.3 2.7 1a9.4 9.4 0 0 1 5 0c1.9-1.3 2.7-1 2.7-1 .5 1.3.2 2.3.1 2.6.6.7 1 1.6 1 2.7 0 3.8-2.4 4.6-4.6 4.9.3.3.6.9.6 1.8v2.7c0 .3.2.6.7.5A10 10 0 0 0 12 2Z" />
      </svg>
    ),
  },
  {
    name: 'X',
    handle: '@meridionaapp',
    href: 'https://x.com/meridionaapp',
    icon: (
      <svg width="15" height="15" viewBox="0 0 24 24" style={{ fill: 'var(--t-title)' }}>
        <path d="M18.2 2.5h3.3l-7.2 8.2 8.5 11.3h-6.7l-5.2-6.9-6 6.9H1.6l7.7-8.8L1.2 2.5H8l4.7 6.3 5.5-6.3Zm-1.2 17.8h1.8L7.1 4.3H5.2l11.8 16Z" />
      </svg>
    ),
  },
]

export function ReportModal({ onClose }: { onClose: () => void }) {
  useEffect(() => {
    function onKey(e: KeyboardEvent) { if (e.key === 'Escape') onClose() }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose])

  return (
    <div className="absolute inset-0 z-40 flex items-center justify-center p-6 sm:p-10 rise"
      style={{ background: 'rgba(20,16,40,0.5)', backdropFilter: 'blur(3px)' }} onClick={onClose}>
      <div className="w-full rounded-[22px] overflow-hidden bg-card"
        style={{ maxWidth: 500, maxHeight: '92%', boxShadow: '0 44px 90px -34px rgba(20,10,60,0.6)' }}
        onClick={e => e.stopPropagation()}>
        {/* hero header — fixed brand gradient, theme-independent */}
        <div className="relative text-center overflow-hidden"
          style={{ padding: '28px 26px 24px', background: 'linear-gradient(135deg,#6D28D9,#9333EA 52%,#DB2777)' }}>
          <div className="absolute rounded-full pointer-events-none"
            style={{ left: -30, top: -30, width: 130, height: 130, background: 'rgba(255,255,255,.14)' }} />
          <div className="absolute rounded-full pointer-events-none"
            style={{ right: -24, bottom: -40, width: 120, height: 120, background: 'rgba(255,255,255,.1)' }} />
          <button onClick={onClose} aria-label="Close"
            className="absolute inline-flex items-center justify-center rounded-full"
            style={{ right: 16, top: 16, width: 30, height: 30, border: 'none', background: 'rgba(255,255,255,.22)', color: '#fff', zIndex: 2 }}>
            <span className="text-[15px] leading-none">×</span>
          </button>
          <div className="relative">
            <div className="mx-auto rounded-[17px] flex items-center justify-center"
              style={{ width: 56, height: 56, background: 'rgba(255,255,255,.16)', border: '1px solid rgba(255,255,255,.28)', fontSize: 26, color: '#fff' }}>
              ✦
            </div>
            <p className="mt-modal-title mt-4" style={{ color: '#fff' }}>You&apos;re shaping Meridian</p>
            <p className="mt-body-sm mx-auto mt-2" style={{ color: 'rgba(255,255,255,.9)', maxWidth: 360 }}>
              Meridian is built by a small team that reads every single message. Found a bug, dreaming up a
              feature, or just want to say hi? We&apos;d genuinely love to hear from you.
            </p>
          </div>
        </div>

        {/* channels */}
        <div style={{ padding: '20px 22px 12px' }}>
          <p className="mt-label" style={{ color: 'var(--t-faint)', marginBottom: 11 }}>REACH US ANYTIME</p>
          <div className="grid grid-cols-2 gap-2.5">
            {CHANNELS.map(c => (
              <a key={c.name} href={c.href} target="_blank" rel="noopener noreferrer"
                className="mt-card-hover flex items-center gap-2.5 no-underline"
                style={{ padding: '12px 13px', borderRadius: 13, border: '1px solid var(--t-card-border)', background: 'var(--t-box)' }}>
                <span className="flex items-center justify-center rounded-[10px] shrink-0"
                  style={{ width: 36, height: 36, background: 'var(--t-wrap)' }}>
                  {c.icon}
                </span>
                <div className="flex-1 min-w-0">
                  <p className="mt-card-title" style={{ color: 'var(--t-title)' }}>{c.name}</p>
                  <p className="mt-body-sm truncate" style={{ color: 'var(--t-faint)', marginTop: 1 }}>{c.handle}</p>
                </div>
                <span style={{ color: 'var(--t-faint-2)', fontWeight: 700, fontSize: 13 }}>↗</span>
              </a>
            ))}
          </div>
        </div>

        {/* quick report */}
        <div style={{ padding: '8px 22px 22px' }}>
          <div className="flex gap-2">
            <a href="mailto:hey@meridiona.com?subject=Bug%20report" className="flex-1 text-center no-underline"
              style={{ border: '1px solid var(--t-ctrl-border)', background: 'var(--t-ctrl)', color: 'var(--t-muted)', borderRadius: 12, padding: 11, font: "700 12.5px 'Plus Jakarta Sans'" }}>
              Report a bug
            </a>
            <a href="mailto:hey@meridiona.com?subject=Feature%20suggestion" className="flex-1 text-center no-underline"
              style={{ border: 'none', background: 'linear-gradient(135deg,#6D28D9,#9333EA)', color: '#fff', borderRadius: 12, padding: 11, font: "700 12.5px 'Plus Jakarta Sans'" }}>
              Suggest a feature
            </a>
          </div>
          <p className="text-center" style={{ font: "500 10.5px 'Plus Jakarta Sans'", color: 'var(--t-faint-2)', marginTop: 12 }}>
            Thank you for helping Meridian get better · we usually reply within a day 💜
          </p>
        </div>
      </div>
    </div>
  )
}
