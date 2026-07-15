//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { CatDot } from 'meridian-design-system'

function Row({ cat, label }: { cat: string; label: string }) {
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
      <CatDot cat={cat} size={8} />
      <span style={{ fontSize: 12 }}>{label}</span>
    </div>
  )
}

export function Coding() {
  return <Row cat="coding" label="Coding" />
}

export function Meeting() {
  return <Row cat="meeting" label="Meeting" />
}

export function AllCategories() {
  const cats: [string, string][] = [
    ['coding', 'Coding'], ['code_review', 'Code review'], ['meeting', 'Meeting'],
    ['communication', 'Comms'], ['design', 'Design'], ['documentation', 'Docs'],
    ['planning', 'Planning'], ['deployment_devops', 'DevOps'], ['research', 'Research'],
    ['idle_personal', 'Idle'],
  ]
  return (
    <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 8 }}>
      {cats.map(([c, label]) => <Row key={c} cat={c} label={label} />)}
    </div>
  )
}
