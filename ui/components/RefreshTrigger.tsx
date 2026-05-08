// meridian — AI activity intelligence by Meridiona

'use client'

import { useEffect } from 'react'
import { useRouter } from 'next/navigation'

export default function RefreshTrigger({ intervalMs = 30_000 }: { intervalMs?: number }) {
  const router = useRouter()
  useEffect(() => {
    const id = setInterval(() => router.refresh(), intervalMs)
    return () => clearInterval(id)
  }, [router, intervalMs])
  return null
}
