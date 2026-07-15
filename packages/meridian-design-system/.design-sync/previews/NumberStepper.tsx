//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { useState } from 'react'
import { NumberStepper } from 'meridian-design-system'

export function Default() {
  const [v, setV] = useState(3)
  return <NumberStepper value={v} onChange={setV} min={0} max={10} />
}

export function AtMin() {
  const [v, setV] = useState(0)
  return <NumberStepper value={v} onChange={setV} min={0} max={10} />
}

export function AtMax() {
  const [v, setV] = useState(10)
  return <NumberStepper value={v} onChange={setV} min={0} max={10} />
}

export function CustomStep() {
  const [v, setV] = useState(15)
  return <NumberStepper value={v} onChange={setV} min={0} max={60} step={5} />
}
