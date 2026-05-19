// meridian — normalises screenpipe activity into structured app sessions
'use client'

import { Minus, Plus } from 'lucide-react'

interface NumberStepperProps {
  value: number
  onChange: (v: number) => void
  min?: number
  max?: number
  step?: number
}

export function NumberStepper({ value, onChange, min, max, step = 1 }: NumberStepperProps) {
  const atMin = min !== undefined && value <= min
  const atMax = max !== undefined && value >= max

  function clamp(n: number) {
    let v = n
    if (min !== undefined && v < min) v = min
    if (max !== undefined && v > max) v = max
    return v
  }

  return (
    <div style={{
      display: 'inline-flex',
      alignItems: 'stretch',
      height: '30px',
      borderRadius: '7px',
      border: '1px solid var(--rule-2)',
      background: 'var(--surface)',
      overflow: 'hidden',
      boxShadow: '0 1px 2px rgba(0,0,0,0.07)',
    }}>
      {/* Decrement */}
      <button
        type="button"
        disabled={atMin}
        onClick={() => onChange(clamp(value - step))}
        className="stepper-btn"
        style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          width: '30px',
          border: 'none',
          cursor: atMin ? 'not-allowed' : 'default',
          color: atMin ? 'var(--ink-4)' : 'var(--ink-2)',
          flexShrink: 0,
        }}
        aria-label="Decrease"
      >
        <Minus size={11} strokeWidth={2.5} />
      </button>

      {/* Divider */}
      <div style={{ width: '1px', background: 'var(--rule)', flexShrink: 0, alignSelf: 'stretch' }} />

      {/* Value input */}
      <input
        type="number"
        value={value}
        min={min}
        max={max}
        step={step}
        onChange={e => {
          const n = Number(e.target.value)
          if (!isNaN(n)) onChange(clamp(n))
        }}
        className="stepper-input"
        style={{
          width: '52px',
          border: 'none',
          background: 'transparent',
          textAlign: 'center',
          fontSize: '13px',
          color: 'var(--ink)',
          outline: 'none',
          padding: '0 2px',
        }}
      />

      {/* Divider */}
      <div style={{ width: '1px', background: 'var(--rule)', flexShrink: 0, alignSelf: 'stretch' }} />

      {/* Increment */}
      <button
        type="button"
        disabled={atMax}
        onClick={() => onChange(clamp(value + step))}
        className="stepper-btn"
        style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          width: '30px',
          border: 'none',
          cursor: atMax ? 'not-allowed' : 'default',
          color: atMax ? 'var(--ink-4)' : 'var(--ink-2)',
          flexShrink: 0,
        }}
        aria-label="Increase"
      >
        <Plus size={11} strokeWidth={2.5} />
      </button>
    </div>
  )
}
