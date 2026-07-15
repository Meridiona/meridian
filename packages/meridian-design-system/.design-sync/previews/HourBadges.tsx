//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { HourBadges } from 'meridian-design-system'

export function PausedNow() {
  return <HourBadges pausedNow pausedHistoric={false} />
}

export function PausedHistoric() {
  return <HourBadges pausedNow={false} pausedHistoric />
}
