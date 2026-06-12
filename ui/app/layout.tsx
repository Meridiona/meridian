//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

import type { Metadata } from 'next'
import { GeistSans } from 'geist/font/sans'
import { GeistMono } from 'geist/font/mono'
import './globals.css'
import { ThemeProvider } from '@/lib/theme-context'
import NoticeBar from '@/components/NoticeBar'

export const metadata: Metadata = {
  title: 'Meridian',
  description: 'Local activity intelligence by Meridiona',
}

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en" className={`${GeistSans.variable} ${GeistMono.variable}`}>
      <body className="min-h-screen font-sans">
        <ThemeProvider>
          <NoticeBar />
          {children}
        </ThemeProvider>
      </body>
    </html>
  )
}
