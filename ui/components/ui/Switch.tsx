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
      // data-state="checked"|"unchecked" set by Radix — CSS in globals.css handles colors + animation
      className="meridian-switch"
      style={{
        width: '42px',
        height: '24px',
        borderRadius: '12px',
        border: 'none',
        padding: 0,
        cursor: 'default',
        flexShrink: 0,
        position: 'relative',
        outline: 'none',
      }}
    >
      <RadixSwitch.Thumb
        className="meridian-switch-thumb"
        style={{
          display: 'block',
          width: '20px',
          height: '20px',
          borderRadius: '50%',
          background: '#fff',
          // layered shadow: first layer is the ambient drop, second is the key light
          boxShadow: '0 2px 4px rgba(0,0,0,0.24), 0 0.5px 1px rgba(0,0,0,0.12)',
          position: 'absolute',
          top: '2px',
          willChange: 'transform',
        }}
      />
    </RadixSwitch.Root>
  )
}
