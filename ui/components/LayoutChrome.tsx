//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Global chrome (external-link interceptor, notice banner, notification
// banner) that the root layout renders around every page. Skipped on the
// setup wizard so the onboarding window stays a self-contained shell — a
// notice/notification firing mid-onboard would land on top of the wizard
// rail and read as a bug, and the wizard's external links are already
// wired through `open_external_url` explicitly.

'use client'

import { usePathname } from 'next/navigation'
import ExternalLinks from '@/components/ExternalLinks'
import NoticeBar from '@/components/NoticeBar'
import NotificationBanner from '@/components/NotificationBanner'

export default function LayoutChrome() {
  const pathname = usePathname()
  if (pathname?.startsWith('/setup')) return null
  return (
    <>
      <ExternalLinks />
      <NoticeBar />
      <NotificationBanner />
    </>
  )
}
