// meridian — AI activity intelligence by Meridiona

import type { Metadata } from 'next'
import { GeistSans } from 'geist/font/sans'
import { GeistMono } from 'geist/font/mono'
import './globals.css'
import Nav from '@/components/Nav'

export const metadata: Metadata = {
  title: 'Meridian',
  description: 'Local activity intelligence by Meridiona',
}

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en" className={`${GeistSans.variable} ${GeistMono.variable}`}>
      <body className="min-h-screen bg-[#F8F7F4] text-[#141414] font-sans">
        <Nav />
        <main className="max-w-4xl mx-auto px-5 pb-16 pt-8">
          {children}
        </main>
      </body>
    </html>
  )
}
