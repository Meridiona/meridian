//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The global system-notice bar (daemon faults — DB errors, permission issues)
// that the root layout renders on every page — gated OFF on the setup wizard
// so the onboarding window stays a self-contained shell. A fault notice
// firing mid-onboard would otherwise land on top of the wizard rail and read
// as a bug. This is the only surface for daemon-fault notices, so it stays
// even though the plan/cleanup nudge banners were removed in favor of the
// sidebar (OverviewPanel's cleanup CTA and Today's-focus plan section).
//
// NOTE: ExternalLinks is deliberately NOT gated here — it's an invisible
// click-interceptor (not chrome) that makes external <a> links work inside the
// Tauri webview. The wizard has external links too (tracker docs, etc.), so it
// stays mounted in the root layout for every route. `external-links.test.ts`
// guards that it covers the wizard.

'use client'

import { usePathname } from 'next/navigation'
import NoticeBar from '@/components/NoticeBar'

export default function LayoutBanners() {
  const pathname = usePathname()
  if (pathname?.startsWith('/setup')) return null
  return <NoticeBar />
}
