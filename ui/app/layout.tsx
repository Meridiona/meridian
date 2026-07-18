//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

import type { Metadata } from 'next'
import { JetBrains_Mono } from 'next/font/google'
import './globals.css'
import { ThemeProvider } from '@/lib/theme-context'
import ExternalLinks from '@/components/ExternalLinks'
import LayoutBanners from '@/components/LayoutBanners'

// One-off exception: --font-jetbrains-mono, used ONLY by the day-task
// timeline's hour-rail labels (DayTaskColumn.tsx), to match the marketing
// site's product demo (meridiona-website/assets/css/demo.css .hour-label),
// which sets real JetBrains Mono rather than the app's aliased --font-mono.
// Scoped deliberately, not a reversal of the decision below.
const jetbrainsMono = JetBrains_Mono({
  weight: ['500', '600'],
  subsets: ['latin'],
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
  description: 'Local activity intelligence by Meridiona',
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
