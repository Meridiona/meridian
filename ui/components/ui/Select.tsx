// meridian — normalises screenpipe activity into structured app sessions
'use client'

import * as RadixSelect from '@radix-ui/react-select'
import { ChevronDown, ChevronUp, Check } from 'lucide-react'

interface SelectProps {
  value: string
  onValueChange: (v: string) => void
  options: { value: string; label: string }[]
  placeholder?: string
}

export function Select({ value, onValueChange, options, placeholder }: SelectProps) {
  return (
    <RadixSelect.Root value={value} onValueChange={onValueChange}>
      <RadixSelect.Trigger
        className="select-trigger"
        style={{
          display: 'inline-flex',
          alignItems: 'center',
          justifyContent: 'space-between',
          gap: '8px',
          minWidth: '120px',
          padding: '6px 10px',
          borderRadius: '7px',
          border: '1px solid var(--rule-2)',
          background: 'var(--surface)',
          color: 'var(--ink)',
          fontSize: '13px',
          cursor: 'default',
          outline: 'none',
          boxShadow: '0 1px 2px rgba(0,0,0,0.06)',
        }}
      >
        <RadixSelect.Value placeholder={placeholder} />
        <RadixSelect.Icon style={{ color: 'var(--ink-3)', flexShrink: 0 }}>
          <ChevronDown size={13} strokeWidth={2} />
        </RadixSelect.Icon>
      </RadixSelect.Trigger>

      <RadixSelect.Portal>
        <RadixSelect.Content
          position="popper"
          sideOffset={4}
          style={{
            background: 'var(--surface)',
            border: '1px solid var(--rule)',
            borderRadius: '9px',
            boxShadow: '0 8px 24px rgba(0,0,0,0.12), 0 2px 6px rgba(0,0,0,0.07)',
            overflow: 'hidden',
            zIndex: 9999,
            minWidth: 'var(--radix-select-trigger-width)',
          }}
        >
          <RadixSelect.ScrollUpButton style={{ display: 'flex', justifyContent: 'center', padding: '4px', color: 'var(--ink-3)' }}>
            <ChevronUp size={12} />
          </RadixSelect.ScrollUpButton>

          <RadixSelect.Viewport style={{ padding: '4px' }}>
            {options.map(opt => (
              <RadixSelect.Item
                key={opt.value}
                value={opt.value}
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  justifyContent: 'space-between',
                  padding: '6px 10px',
                  borderRadius: '5px',
                  fontSize: '13px',
                  color: 'var(--ink)',
                  cursor: 'default',
                  outline: 'none',
                  userSelect: 'none',
                }}
                onMouseEnter={e => (e.currentTarget.style.background = 'var(--surface-2)')}
                onMouseLeave={e => (e.currentTarget.style.background = 'transparent')}
                onFocus={e => (e.currentTarget.style.background = 'var(--surface-2)')}
                onBlur={e => (e.currentTarget.style.background = 'transparent')}
              >
                <RadixSelect.ItemText>{opt.label}</RadixSelect.ItemText>
                <RadixSelect.ItemIndicator style={{ color: 'var(--accent)' }}>
                  <Check size={12} strokeWidth={2.5} />
                </RadixSelect.ItemIndicator>
              </RadixSelect.Item>
            ))}
          </RadixSelect.Viewport>

          <RadixSelect.ScrollDownButton style={{ display: 'flex', justifyContent: 'center', padding: '4px', color: 'var(--ink-3)' }}>
            <ChevronDown size={12} />
          </RadixSelect.ScrollDownButton>
        </RadixSelect.Content>
      </RadixSelect.Portal>
    </RadixSelect.Root>
  )
}
