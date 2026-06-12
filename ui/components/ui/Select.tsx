//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

import * as RadixSelect from '@radix-ui/react-select'
import { Check } from 'lucide-react'

interface SelectProps {
  value: string
  onValueChange: (v: string) => void
  options: { value: string; label: string }[]
  placeholder?: string
}

// Up/down double-chevron — the macOS native select badge icon
function SelectChevrons() {
  return (
    <svg viewBox="0 0 10 16" width="8" height="11" fill="white" aria-hidden>
      <path d="M5 4L1 8h8L5 4zm0 8L1 8h8l-4 4z" />
    </svg>
  )
}

export function Select({ value, onValueChange, options, placeholder }: SelectProps) {
  return (
    <RadixSelect.Root value={value} onValueChange={onValueChange}>
      <RadixSelect.Trigger
        className="select-trigger"
        style={{
          display: 'inline-flex',
          alignItems: 'center',
          gap: 0,
          borderRadius: '6px',
          border: '1px solid var(--rule-2)',
          background: 'var(--surface)',
          fontSize: '13px',
          color: 'var(--ink)',
          cursor: 'pointer',
          outline: 'none',
          overflow: 'hidden',
          boxShadow: '0 1px 2px rgba(0,0,0,0.07)',
          minWidth: '120px',
        }}
      >
        {/* Label area */}
        <span style={{ flex: 1, padding: '6px 8px 6px 10px', textAlign: 'left' }}>
          <RadixSelect.Value placeholder={placeholder} />
        </span>

        {/* macOS accent badge with up/down chevrons */}
        <RadixSelect.Icon asChild>
          <span style={{
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            width: '24px',
            alignSelf: 'stretch',
            background: 'var(--accent)',
            flexShrink: 0,
          }}>
            <SelectChevrons />
          </span>
        </RadixSelect.Icon>
      </RadixSelect.Trigger>

      <RadixSelect.Portal>
        <RadixSelect.Content
          position="popper"
          sideOffset={5}
          style={{
            background: 'var(--surface)',
            border: '1px solid var(--rule)',
            borderRadius: '9px',
            boxShadow: '0 8px 32px rgba(0,0,0,0.14), 0 2px 8px rgba(0,0,0,0.08)',
            overflow: 'hidden',
            zIndex: 9999,
            minWidth: 'var(--radix-select-trigger-width)',
          }}
        >
          <RadixSelect.Viewport style={{ padding: '4px' }}>
            {options.map(opt => (
              <RadixSelect.Item
                key={opt.value}
                value={opt.value}
                className="select-item"
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  justifyContent: 'space-between',
                  padding: '6px 10px',
                  borderRadius: '5px',
                  fontSize: '13px',
                  cursor: 'pointer',
                  outline: 'none',
                  userSelect: 'none',
                  color: 'var(--ink)',
                }}
              >
                <RadixSelect.ItemText>{opt.label}</RadixSelect.ItemText>
                <RadixSelect.ItemIndicator className="select-check" style={{ color: 'var(--accent)', marginLeft: '8px' }}>
                  <Check size={12} strokeWidth={2.5} />
                </RadixSelect.ItemIndicator>
              </RadixSelect.Item>
            ))}
          </RadixSelect.Viewport>
        </RadixSelect.Content>
      </RadixSelect.Portal>
    </RadixSelect.Root>
  )
}
