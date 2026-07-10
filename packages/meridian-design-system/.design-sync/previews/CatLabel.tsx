//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { CatLabel } from 'meridian-design-system'

export function Coding() {
  return <CatLabel cat="coding" />
}

export function CodeReview() {
  return <CatLabel cat="code_review" />
}

export function Stack() {
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
      <CatLabel cat="meeting" />
      <CatLabel cat="planning" />
      <CatLabel cat="deployment_devops" />
    </div>
  )
}
