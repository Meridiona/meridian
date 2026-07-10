//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { EditableTitle } from 'meridian-design-system'

export function Default() {
  return (
    <div style={{ width: 320 }}>
      <EditableTitle value="Add retry with backoff to hf-proxy fetch" busy={false} onSave={() => {}} />
    </div>
  )
}

export function LongTitle() {
  return (
    <div style={{ width: 320 }}>
      <EditableTitle
        value="Investigate intermittent OAuth refresh-token race on the Jira integration"
        busy={false}
        onSave={() => {}}
      />
    </div>
  )
}
