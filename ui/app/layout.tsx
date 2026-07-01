//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

import type { Metadata } from 'next'
import { GeistSans } from 'geist/font/sans'
import { GeistMono } from 'geist/font/mono'
import { Instrument_Serif, Plus_Jakarta_Sans, JetBrains_Mono } from 'next/font/google'
import './globals.css'
import { ThemeProvider } from '@/lib/theme-context'
import NoticeBar from '@/components/NoticeBar'
import NotificationBanner from '@/components/NotificationBanner'

const instrumentSerif = Instrument_Serif({
  weight: '400',
  style: ['normal', 'italic'],
  subsets: ['latin'],
  variable: '--font-instrument-serif',
  display: 'swap',
})

// Meridian Timeline design — the app's primary UI font (replaces GeistSans as
// --font-sans in globals.css). GeistSans/GeistMono stay loaded for the two
// remaining direct --font-geist-mono consumers (NoticeBar, LogsView) and the
// unreachable Sessions/Week routes.
const plusJakartaSans = Plus_Jakarta_Sans({
  weight: ['400', '500', '600', '700', '800'],
  subsets: ['latin'],
  variable: '--font-pjs',
  display: 'swap',
})

const jetBrainsMono = JetBrains_Mono({
  weight: ['400', '500', '600', '700'],
  subsets: ['latin'],
  variable: '--font-jbmono',
  display: 'swap',
})

export const metadata: Metadata = {
  title: 'Meridian',
  description: 'Local activity intelligence by Meridiona',
}

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html
      lang="en"
      className={`${GeistSans.variable} ${GeistMono.variable} ${instrumentSerif.variable} ${plusJakartaSans.variable} ${jetBrainsMono.variable}`}
    >
      <body className="min-h-screen font-sans">
        <ThemeProvider>
          <NoticeBar />
          <NotificationBanner />
          {children}
        </ThemeProvider>
      </body>
    </html>
  )
}
