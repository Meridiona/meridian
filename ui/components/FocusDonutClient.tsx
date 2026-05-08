// meridian — AI activity intelligence by Meridiona

'use client'

import dynamic from 'next/dynamic'
import type { StatsResponse } from '@/lib/types'

const FocusDonut = dynamic(() => import('@/components/FocusDonut'), { ssr: false })

interface Props {
  apps: StatsResponse['top_apps']
  focusS: number
  idleS: number
}

export default function FocusDonutClient(props: Props) {
  return <FocusDonut {...props} />
}
