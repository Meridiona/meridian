// meridian — normalises screenpipe activity into structured app sessions
'use client'

import { useRef } from 'react'
import { Minus, Plus } from 'lucide-react'

interface NumberStepperProps {
  value: number
  onChange: (v: number) => void
  min?: number
  max?: number
  step?: number
}

export function NumberStepper({ value, onChange, min, max, step = 1 }: NumberStepperProps) {
  const inputRef = useRef<HTMLInputElement>(null)
  const atMin = min !== undefined && value <= min
  const atMax = max !== undefined && value >= max

  function clamp(n: number) {
    if (min !== undefined && n < min) return min
    if (max !== undefined && n > max) return max
    return n
  }

  const btnStyle = (disabled: boolean): React.CSSProperties => ({
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'center',
    width: '30px',
    height: '30px',
    border: 'none',
    background: 'transparent',
    color: disabled ? 'var(--ink-4)' : 'var(--ink-2)',
    cursor: disabled ? 'not-allowed' : 'default',
    flexShrink: 0,
    transition: 'color 0.1s',
  })

  return (
    <div style={{
      display: 'inline-flex',
      alignItems: 'center',
      height: '30px',
      borderRadius: '7px',
      border: '1px solid var(--rule-2)',
      background: 'var(--surface)',
      overflow: 'hidden',
      boxShadow: '0 1px 2px rgba(0,0,0,0.06)',
    }}>
      <button type="button" disabled={atMin} onClick={() => onChange(clamp(value - step))} style={btnStyle(atMin)}>
        <Minus size={11} strokeWidth={2.5} />
      </button>

      <div style={{ width: '1px', height: '16px', background: 'var(--rule)', flexShrink: 0 }} />

      <input
        ref={inputRef}
        type="number"
        value={value}
        min={min}
        max={max}
        step={step}
        onChange={e => {
          const n = Number(e.target.value)
          if (!isNaN(n)) onChange(clamp(n))
        }}
        style={{
          width: '52px',
          height: '100%',
          border: 'none',
          background: 'transparent',
          textAlign: 'center',
          fontSize: '13px',
          color: 'var(--ink)',
          outline: 'none',
          padding: '0 2px',
          // spinner hidden via globals.css
        }}
        className="stepper-input"
      />

      <div style={{ width: '1px', height: '16px', background: 'var(--rule)', flexShrink: 0 }} />

      <button type="button" disabled={atMax} onClick={() => onChange(clamp(value + step))} style={btnStyle(atMax)}>
        <Plus size={11} strokeWidth={2.5} />
      </button>
    </div>
  )
}
