//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The two VISIBLE global banners (system-notice bar + notification banner) that
// the root layout renders on every page — gated OFF on the setup wizard so the
// onboarding window stays a self-contained shell. A fault notice or a
// notification firing mid-onboard would otherwise land on top of the wizard
// rail and read as a bug.
//
// NOTE: ExternalLinks is deliberately NOT gated here — it's an invisible
// click-interceptor (not chrome) that makes external <a> links work inside the
// Tauri webview. The wizard has external links too (tracker docs, etc.), so it
// stays mounted in the root layout for every route. `external-links.test.ts`
// guards that it covers the wizard.

'use client'

import { usePathname } from 'next/navigation'
import NoticeBar from '@/components/NoticeBar'
import NotificationBanner from '@/components/NotificationBanner'

export default function LayoutBanners() {
  const pathname = usePathname()
  if (pathname?.startsWith('/setup')) return null
  return (
    <>
      <NoticeBar />
      <NotificationBanner />
    </>
  )
}
