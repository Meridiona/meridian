//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { ProviderIcon } from 'meridian-design-system'

function Row({ provider, label }: { provider: string; label: string }) {
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
      <ProviderIcon provider={provider} size={18} />
      <span style={{ fontSize: 12 }}>{label}</span>
    </div>
  )
}

export function Jira() {
  return <Row provider="jira" label="Jira" />
}

export function AllProviders() {
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
      <Row provider="jira" label="Jira" />
      <Row provider="linear" label="Linear" />
      <Row provider="github" label="GitHub" />
      <Row provider="trello" label="Trello" />
      <Row provider="azure_devops" label="Azure DevOps" />
    </div>
  )
}
