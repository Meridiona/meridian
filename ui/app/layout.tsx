//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

import type { Metadata } from 'next'
import localFont from 'next/font/local'
import './globals.css'
import { ThemeProvider } from '@/lib/theme-context'
import ExternalLinks from '@/components/ExternalLinks'
import LayoutBanners from '@/components/LayoutBanners'

// One-off exception: --font-jetbrains-mono, used ONLY by the day-task
// timeline's hour-rail labels (DayTaskColumn.tsx), to match the marketing
// site's product demo (meridiona-website/assets/css/demo.css .hour-label),
// which sets real JetBrains Mono rather than the app's aliased --font-mono.
// Scoped deliberately, not a reversal of the decision below.
//
// VENDORED (`next/font/local`), NOT `next/font/google`. The loader fetches from
// fonts.gstatic.com AT BUILD TIME, which put a live network dependency on
// Google in every `next build` - including CI and the release job. That is not
// theoretical: on 2026-08-16 the CI UI job failed with six `Received response
// with status 404 ... /jetbrainsmono/v24/...woff2` errors and `Turbopack build
// failed with 12 errors`, while the same commit built fine locally. Google had
// rotated the v24 file hashes; a machine with a warm font cache never noticed,
// a cold CI runner could not resolve a single file. Nothing in the repo had
// changed, and no re-run would have fixed it.
//
// The file is the VARIABLE latin subset, which is why one 55KB file covers both
// weights the design uses (500 and 600) - `weight: '100 800'` is the axis range
// JetBrains Mono ships, not a request for every weight. Only `latin` is
// vendored: the Google loader also pulled cyrillic/greek/vietnamese subsets,
// and the hour-rail labels this serves are digits and `AM`/`PM`.
//
// Licensed under the SIL Open Font License 1.1 - see `./fonts/OFL.txt`, kept
// next to the file because the OFL requires the notice to travel with it.
// Refreshing it means re-downloading from the same URL family and updating
// nothing else; the API here does not change.
const jetbrainsMono = localFont({
  src: './fonts/JetBrainsMono-latin.woff2',
  weight: '100 800',
  style: 'normal',
  variable: '--font-jetbrains-mono',
  display: 'swap',
})

// Meridian Timeline design's single voice is SF Pro, via --font-sans in
// globals.css's system-font stack (-apple-system/system-ui resolve to the real
// SF Pro on macOS) — no next/font loader needed. Plus Jakarta Sans + JetBrains
// Mono were retired in favor of one face for both UI text and numerics (the
// scoped hour-rail exception above aside), and the setup/uninstall wizards'
// Instrument Serif hero font was retired alongside them — both wizards now use
// the same SF Pro display treatment as the timeline.

export const metadata: Metadata = {
  title: 'Meridian',
  description: 'A private daily timeline of your work, plus auto-drafted worklogs for your tasks - ready to review and post to Jira or GitHub.',
}

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en" className={jetbrainsMono.variable}>
      <body className="min-h-screen font-sans">
        <ThemeProvider>
          {/* ExternalLinks is an invisible interceptor — mounted for every
              route (wizard included) so external links work in the webview.
              Only the visible banners are gated off the wizard. */}
          <ExternalLinks />
          <LayoutBanners />
          {children}
        </ThemeProvider>
      </body>
    </html>
  )
}
