//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { useState } from 'react'
import { Select } from 'meridian-design-system'

const TRACKER_OPTIONS = [
  { value: 'jira', label: 'Jira' },
  { value: 'linear', label: 'Linear' },
  { value: 'github', label: 'GitHub' },
  { value: 'trello', label: 'Trello' },
  { value: 'azure_devops', label: 'Azure DevOps' },
]

export function Default() {
  const [v, setV] = useState('jira')
  return <Select value={v} onValueChange={setV} options={TRACKER_OPTIONS} />
}

export function WithPlaceholder() {
  const [v, setV] = useState('')
  return <Select value={v} onValueChange={setV} options={TRACKER_OPTIONS} placeholder="Choose a tracker…" />
}
