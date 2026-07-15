//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { useState } from 'react'
import { TextInput } from 'meridian-design-system'

export function Default() {
  const [v, setV] = useState('')
  return <TextInput value={v} onChange={setV} placeholder="ATATT3xFfGF0…" />
}

export function Filled() {
  const [v, setV] = useState('sk-example-xxxxxxxxxxxxxxxx')
  return <TextInput value={v} onChange={setV} placeholder="API key" />
}

export function Password() {
  const [v, setV] = useState('correct horse battery staple')
  return <TextInput value={v} onChange={setV} type="password" placeholder="Password" />
}

export function TimeField() {
  const [v, setV] = useState('09:00')
  return <TextInput value={v} onChange={setV} type="time" width={140} />
}
