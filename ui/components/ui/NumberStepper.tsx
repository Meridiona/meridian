//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

import { Minus, Plus } from 'lucide-react'
import { useState, useEffect } from 'react'

interface NumberStepperProps {
  value: number
  onChange: (v: number) => void
  min?: number
  max?: number
  step?: number
}

export function NumberStepper({ value, onChange, min, max, step = 1 }: NumberStepperProps) {
  const [inputVal, setInputVal] = useState(String(value))
  const [focused, setFocused] = useState(false)

  // Keep display in sync when value changes externally (e.g. stepper buttons)
  useEffect(() => {
    if (!focused) setInputVal(String(value))
  }, [value, focused])

  const atMin = min !== undefined && value <= min
  const atMax = max !== undefined && value >= max

  function clamp(n: number) {
    let v = n
    if (min !== undefined && v < min) v = min
    if (max !== undefined && v > max) v = max
    return v
  }

  function commit(raw: string) {
    const n = Number(raw)
    if (!isNaN(n) && raw.trim() !== '') {
      onChange(clamp(n))
    } else {
      // revert display to last valid value
      setInputVal(String(value))
    }
  }

  return (
    <div style={{
      display: 'inline-flex',
      alignItems: 'stretch',
      height: '30px',
      borderRadius: '7px',
      border: '1px solid var(--t-input-border)',
      background: 'var(--t-input)',
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
          color: atMin ? 'var(--t-faint-2)' : 'var(--t-muted)',
          flexShrink: 0,
        }}
        aria-label="Decrease"
      >
        <Minus size={11} strokeWidth={2.5} />
      </button>

      {/* Divider */}
      <div style={{ width: '1px', background: 'var(--t-hair)', flexShrink: 0, alignSelf: 'stretch' }} />

      {/* Value input — local string state so partial edits aren't clamped mid-type */}
      <input
        type="number"
        value={inputVal}
        min={min}
        max={max}
        step={step}
        onChange={e => setInputVal(e.target.value)}
        onFocus={() => setFocused(true)}
        onBlur={() => { setFocused(false); commit(inputVal) }}
        onKeyDown={e => { if (e.key === 'Enter') commit(inputVal) }}
        className="stepper-input"
        style={{
          width: '52px',
          border: 'none',
          background: 'transparent',
          textAlign: 'center',
          fontSize: '13px',
          color: 'var(--t-title)',
          outline: 'none',
          padding: '0 2px',
        }}
      />

      {/* Divider */}
      <div style={{ width: '1px', background: 'var(--t-hair)', flexShrink: 0, alignSelf: 'stretch' }} />

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
          color: atMax ? 'var(--t-faint-2)' : 'var(--t-muted)',
          flexShrink: 0,
        }}
        aria-label="Increase"
      >
        <Plus size={11} strokeWidth={2.5} />
      </button>
    </div>
  )
}
