//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
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
