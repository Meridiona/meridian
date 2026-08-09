//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// "Report" / get-in-touch modal, ported from the Meridian Timeline design
// mock (Claude Design project b8656e29-ae04-4f69-b17f-d5fab4d00f3a, the
// REPORT / GET IN TOUCH MODAL block) opened from the toolbar nav pill's
// "Report" item (see Toolbar.tsx's MeridianNavPill). The hero header IS the nav
// pill's dark lockup (`--mer-pill-bg`) — deliberately independent of the
// light/blush/ink surface theme — everything
// below it (channel cards, buttons, footer) uses the live `--t-*` tokens so it
// matches whichever theme is active. Overlay/backdrop/Escape-to-close mirrors
// ModalShell; the hero header replaces ModalShell's plain title bar so this
// doesn't wrap it.

'use client'

import { useEffect } from 'react'

import { MeridianMark } from './Toolbar'

/** The public repo the in-app links point at - issues, releases, and the two
 *  report buttons below. One constant so the four call sites cannot drift onto
 *  different orgs, which is how the old `github.com/meridiona` (no repo, wrong
 *  case) survived. */
const REPO = 'https://github.com/Meridiona/meridian'

/** Deep link to a prefilled new-issue form. `template` is a file name under
 *  `.github/ISSUE_TEMPLATE/`. */
const ISSUE_URL = (template: string) => `${REPO}/issues/new?template=${template}`

// TWO CHANNELS, NOT FOUR. Discord (`discord.gg/meridiona`) and X
// (`@meridionaapp`) were listed before either existed, so both links landed on
// a dead invite and an empty profile - a contact screen whose whole promise is
// "a person reads every message" is the worst place in the product to have a
// link that goes nowhere. They come back when there is something behind them.
const CHANNELS: {
  name: string
  handle: string
  href: string
  icon: React.ReactNode
}[] = [
  {
    name: 'Email',
    handle: 'company@meridiona.com',
    href: 'mailto:company@meridiona.com',
    icon: (
      <svg width="17" height="17" viewBox="0 0 24 24" fill="none">
        <rect x="2.5" y="5" width="19" height="14" rx="3" stroke="#EA4335" strokeWidth="1.8" />
        <path d="M3.5 6.5 L12 12.5 L20.5 6.5" stroke="#EA4335" strokeWidth="1.8" strokeLinecap="round" />
      </svg>
    ),
  },
  {
    name: 'GitHub',
    handle: 'Issues & releases',
    href: `${REPO}/issues`,
    icon: (
      <svg width="18" height="18" viewBox="0 0 24 24" style={{ fill: 'var(--t-title)' }}>
        <path d="M12 2A10 10 0 0 0 8.8 21.5c.5.1.7-.2.7-.5v-1.7c-2.8.6-3.4-1.3-3.4-1.3-.5-1.2-1.1-1.5-1.1-1.5-.9-.6.1-.6.1-.6 1 .1 1.5 1 1.5 1 .9 1.5 2.3 1.1 2.9.8.1-.6.3-1.1.6-1.3-2.2-.3-4.6-1.1-4.6-4.9 0-1.1.4-2 1-2.7-.1-.3-.4-1.3.1-2.6 0 0 .8-.3 2.7 1a9.4 9.4 0 0 1 5 0c1.9-1.3 2.7-1 2.7-1 .5 1.3.2 2.3.1 2.6.6.7 1 1.6 1 2.7 0 3.8-2.4 4.6-4.6 4.9.3.3.6.9.6 1.8v2.7c0 .3.2.6.7.5A10 10 0 0 0 12 2Z" />
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
        {/* Hero header - Meridian's own backdrop (--mer-brand-bg), theme-independent.
            NOT --draft-card-bg, which this used to be. That token is the fill of
            an AI-drafted card, and in Meridian violet has one meaning: a model
            wrote this. Painting "get in touch with the team" in it said the
            opposite of what the screen is for - the whole point of the copy is
            that a person reads every message.

            THE TWO WHITE DISCS ARE GONE. This carried a 130px and a 120px flat
            white circle bled off opposite corners at 14% and 10% - the standard
            2015 "decorated header" move, and the reason the panel looked dated
            even after the colour was fixed. They are flat (one alpha edge to
            edge), they are hard-edged (a disc you can trace is decoration, not
            light), and they fought the gradient underneath instead of belonging
            to it. The lighting now lives in the token as soft off-centre
            radials, so it reads as a lit surface rather than a rectangle with
            shapes stuck on it. */}
        <div className="relative text-center overflow-hidden"
          style={{
            padding: '30px 26px 26px',
            background: 'var(--mer-brand-bg)',
            boxShadow: 'var(--mer-brand-edge)',
          }}>
          <button onClick={onClose} aria-label="Close"
            className="mt-card-hover absolute inline-flex items-center justify-center rounded-full"
            style={{ right: 16, top: 16, width: 30, height: 30, border: '1px solid rgba(255,255,255,.16)', background: 'rgba(255,255,255,.10)', color: '#fff', zIndex: 2 }}>
            <span className="text-[15px] leading-none">×</span>
          </button>
          <div className="relative">
            {/* Glass, not a flat white square: a translucent fill under a
                brighter top-edge highlight, so the chip picks up the same light
                the surface behind it does. The mark is the app logo rather than
                a ✦ - a sparkle is this product's AI signifier, and it was
                sitting at the top of the one screen that is explicitly about
                reaching a human. Via MeridianMark, not a raw <img>: the source
                PNG carries app-icon safe-area padding, so anything that does not
                apply its 166% crop renders a mark floating in dead space. */}
            <div className="mx-auto rounded-[17px] flex items-center justify-center overflow-hidden"
              style={{
                width: 56, height: 56,
                background: 'rgba(255,255,255,.10)',
                border: '1px solid rgba(255,255,255,.20)',
                boxShadow: 'inset 0 1px 0 rgba(255,255,255,.22), 0 8px 20px -10px rgba(0,0,0,.6)',
              }}>
              <MeridianMark size={30} />
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

        {/* Quick report. These two open the REPO'S OWN issue forms
            (.github/ISSUE_TEMPLATE/), not a mailto - the templates already exist
            and already ask the questions a useful report needs, which an empty
            mail draft does not. It also means a report is searchable, dedupes
            against the ones already filed, and the person who filed it can
            follow the fix; a mail thread gives none of that and has to be
            triaged by hand. Keep these template file names in step with that
            directory - GitHub silently falls back to the template CHOOSER on an
            unknown `template=`, which still works but drops the label and the
            prefilled form. */}
        <div style={{ padding: '8px 22px 22px' }}>
          <div className="flex gap-2">
            <a href={ISSUE_URL('bug_report.yml')} target="_blank" rel="noopener noreferrer"
              className="flex-1 text-center no-underline"
              style={{ border: '1px solid var(--t-ctrl-border)', background: 'var(--t-ctrl)', color: 'var(--t-muted)', borderRadius: 12, padding: 11, font: "700 12.5px var(--font-sans)" }}>
              Report a bug
            </a>
            <a href={ISSUE_URL('feature_request.yml')} target="_blank" rel="noopener noreferrer"
              className="flex-1 text-center no-underline"
              style={{ border: 'none', background: 'var(--btn-primary-bg)', color: '#fff', borderRadius: 12, padding: 11, font: "700 12.5px var(--font-sans)" }}>
              Suggest a feature
            </a>
          </div>
          <p className="text-center" style={{ font: "500 10.5px var(--font-sans)", color: 'var(--t-faint-2)', marginTop: 12 }}>
            Thank you for helping Meridian get better · we usually reply within a day
          </p>
        </div>
      </div>
    </div>
  )
}
