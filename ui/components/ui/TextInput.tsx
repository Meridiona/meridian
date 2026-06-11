// meridian — normalises screenpipe activity into structured app sessions
'use client'

interface TextInputProps {
  value: string
  onChange: (v: string) => void
  placeholder?: string
  type?: 'text' | 'password' | 'email'
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
        background: 'var(--bg)',
        color: 'var(--ink)',
        border: '1px solid var(--rule)',
        borderRadius: '6px',
        outline: 'none',
        fontFamily: 'inherit',
      }}
      onFocus={e => (e.target.style.borderColor = 'var(--accent)')}
      onBlur={e => (e.target.style.borderColor = 'var(--rule)')}
    />
  )
}
