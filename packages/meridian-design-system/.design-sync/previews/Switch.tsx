//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { useState } from 'react'
import { Switch } from 'meridian-design-system'

export function On() {
  const [checked, setChecked] = useState(true)
  return <Switch checked={checked} onCheckedChange={setChecked} id="notifications" />
}

export function Off() {
  const [checked, setChecked] = useState(false)
  return <Switch checked={checked} onCheckedChange={setChecked} id="launch-at-login" />
}
