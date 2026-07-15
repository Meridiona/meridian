//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { EditableSummary } from 'meridian-design-system'

export function Default() {
  return (
    <div style={{ width: 360 }}>
      <EditableSummary
        value="Closed a gap-detection bug where a sleep spanning an ETL run boundary was silently dropped."
        placeholder="Describe what you worked on…"
        busy={false}
        onSave={() => {}}
        onCancel={() => {}}
      />
    </div>
  )
}

export function CustomLabel() {
  return (
    <div style={{ width: 360 }}>
      <EditableSummary
        label="Reasoning"
        value="High OCR + window-title overlap with MER-482 across the full hour."
        placeholder="Why was this matched?"
        busy={false}
        rows={4}
        onSave={() => {}}
        onCancel={() => {}}
        saveLabel="Update reasoning"
      />
    </div>
  )
}

export function Busy() {
  return (
    <div style={{ width: 360 }}>
      <EditableSummary
        value="Saving your edit…"
        placeholder="Describe what you worked on…"
        busy
        onSave={() => {}}
        onCancel={() => {}}
      />
    </div>
  )
}
