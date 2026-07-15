//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

import type { Metadata } from 'next'
import { Instrument_Serif } from 'next/font/google'
import './globals.css'
import { ThemeProvider } from '@/lib/theme-context'
import ExternalLinks from '@/components/ExternalLinks'
import LayoutBanners from '@/components/LayoutBanners'

const instrumentSerif = Instrument_Serif({
  weight: '400',
  style: ['normal', 'italic'],
  subsets: ['latin'],
  variable: '--font-instrument-serif',
  display: 'swap',
})

// Meridian Timeline design's single voice is SF Pro, via --font-sans in
// globals.css's system-font stack (-apple-system/system-ui resolve to the real
// SF Pro on macOS) — no next/font loader needed. Plus Jakarta Sans + JetBrains
// Mono were retired in favor of one face for both UI text and numerics.

export const metadata: Metadata = {
  title: 'Meridian',
  description: 'Local activity intelligence by Meridiona',
}

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html
      lang="en"
      className={instrumentSerif.variable}
    >
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
