//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

interface TextInputProps {
  value: string
  onChange: (v: string) => void
  placeholder?: string
  type?: 'text' | 'password' | 'email' | 'time'
  width?: number | string
}

export function TextInput({ value, onChange, placeholder, type = 'text', width = 220 }: TextInputProps) {
  return (
    <input
      type={type}
      value={value}
      onChange={e => onChange(e.target.value)}
      placeholder={placeholder}
      style={{
        width,
        fontSize: '12px',
        padding: '5px 8px',
        background: 'var(--t-input)',
        color: 'var(--t-title)',
        border: '1px solid var(--t-input-border)',
        borderRadius: '6px',
        outline: 'none',
        fontFamily: 'inherit',
      }}
      onFocus={e => (e.target.style.borderColor = 'var(--color-state-proposal)')}
      onBlur={e => (e.target.style.borderColor = 'var(--t-input-border)')}
    />
  )
}
