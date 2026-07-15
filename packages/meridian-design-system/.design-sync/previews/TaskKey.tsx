//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { TaskKey } from 'meridian-design-system'

export function Default() {
  return <TaskKey keyId="MER-482" />
}

export function Big() {
  return <TaskKey keyId="MER-501" big />
}

export function GitHubStyle() {
  return <TaskKey keyId="meridiona/meridian#423" />
}
