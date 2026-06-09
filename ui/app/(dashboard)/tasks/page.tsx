// meridian — normalises screenpipe activity into structured app sessions
'use client'

import { Suspense } from 'react'
import { useSearchParams } from 'next/navigation'
import TasksView from '@/components/views/TasksView'

function TasksContent() {
  const params = useSearchParams()
  return <TasksView focusKey={params.get('focus')} />
}

export default function TasksPage() {
  return (
    <Suspense>
      <TasksContent />
    </Suspense>
  )
}
