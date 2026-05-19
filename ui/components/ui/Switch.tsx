// meridian — normalises screenpipe activity into structured app sessions
'use client'

import * as RadixSwitch from '@radix-ui/react-switch'

interface SwitchProps {
  checked: boolean
  onCheckedChange: (v: boolean) => void
  id?: string
}

export function Switch({ checked, onCheckedChange, id }: SwitchProps) {
  return (
    <RadixSwitch.Root
      id={id}
      checked={checked}
      onCheckedChange={onCheckedChange}
      style={{
        width: '42px',
        height: '24px',
        borderRadius: '12px',
        border: 'none',
        padding: 0,
        cursor: 'default',
        flexShrink: 0,
        background: checked ? 'var(--accent)' : 'var(--rule-2)',
        transition: 'background 0.2s ease',
        position: 'relative',
        outline: 'none',
      }}
    >
      <RadixSwitch.Thumb
        style={{
          display: 'block',
          width: '20px',
          height: '20px',
          borderRadius: '50%',
          background: '#fff',
          boxShadow: '0 1px 4px rgba(0,0,0,0.22)',
          position: 'absolute',
          top: '2px',
          left: checked ? '20px' : '2px',
          transition: 'left 0.18s cubic-bezier(0.25,0.46,0.45,0.94)',
          willChange: 'left',
        }}
      />
    </RadixSwitch.Root>
  )
}
