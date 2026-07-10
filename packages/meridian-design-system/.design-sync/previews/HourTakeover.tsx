//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { HourTakeover } from 'meridian-design-system'

export function Queued() {
  return <HourTakeover hour={14} mode="queued" paused={false} nextHourLabel="3:00 PM" />
}

export function Generating() {
  return <HourTakeover hour={14} mode="generating" paused={false} nextHourLabel="3:00 PM" />
}

export function SoloGenerating() {
  return <HourTakeover hour={9} mode="generating" paused={false} nextHourLabel="10:00 AM" isSolo />
}

export function QueuedAndPaused() {
  return <HourTakeover hour={16} mode="queued" paused nextHourLabel="5:00 PM" />
}
