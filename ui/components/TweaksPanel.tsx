// meridian — normalises screenpipe activity into structured app sessions
'use client'

import { useRef, useState } from 'react'
import { useTheme, ACCENT_PRESETS } from '@/lib/theme-context'

function hexIsLight(hex: string): boolean {
  const h = hex.replace('#', '').padEnd(6, '0')
  const n = parseInt(h.slice(0, 6), 16)
  if (Number.isNaN(n)) return true
  const r = (n >> 16) & 255, g = (n >> 8) & 255, b = n & 255
  return r * 299 + g * 587 + b * 114 > 148000
}

export default function TweaksPanel() {
  const { dark, setDark, accent, setAccent, density, setDensity, tone, setTone } = useTheme()
  const [open, setOpen] = useState(false)
  const dragRef = useRef<HTMLDivElement>(null)
  const offsetRef = useRef({ x: 16, y: 16 })
  const PAD = 16

  function clampToViewport() {
    const panel = dragRef.current
    if (!panel) return
    const w = panel.offsetWidth, h = panel.offsetHeight
    const maxRight  = Math.max(PAD, window.innerWidth - w - PAD)
    const maxBottom = Math.max(PAD, window.innerHeight - h - PAD)
    offsetRef.current = {
      x: Math.min(maxRight,  Math.max(PAD, offsetRef.current.x)),
      y: Math.min(maxBottom, Math.max(PAD, offsetRef.current.y)),
    }
    panel.style.right  = offsetRef.current.x + 'px'
    panel.style.bottom = offsetRef.current.y + 'px'
  }

  function onDragStart(e: React.MouseEvent) {
    const panel = dragRef.current
    if (!panel) return
    const r = panel.getBoundingClientRect()
    const sx = e.clientX, sy = e.clientY
    const startRight  = window.innerWidth  - r.right
    const startBottom = window.innerHeight - r.bottom
    const move = (ev: MouseEvent) => {
      offsetRef.current = { x: startRight - (ev.clientX - sx), y: startBottom - (ev.clientY - sy) }
      clampToViewport()
    }
    const up = () => {
      window.removeEventListener('mousemove', move)
      window.removeEventListener('mouseup', up)
    }
    window.addEventListener('mousemove', move)
    window.addEventListener('mouseup', up)
  }

  return (
    <>
      {/* Trigger button */}
      <button
        onClick={() => setOpen(o => !o)}
        className="fixed right-4 bottom-4 z-40 w-9 h-9 rounded-full flex items-center justify-center text-[13px]"
        style={{
          background: 'var(--surface)',
          border: '1px solid var(--rule-2)',
          color: 'var(--ink-3)',
          boxShadow: '0 2px 8px rgba(0,0,0,0.1)',
        }}
        aria-label="Open theme settings"
      >
        ◐
      </button>

      {open && (
        <div
          ref={dragRef}
          className="fixed z-50 w-[280px] rounded-[14px] overflow-hidden flex flex-col"
          style={{
            right: offsetRef.current.x,
            bottom: offsetRef.current.y,
            background: dark ? 'rgba(20,20,19,0.92)' : 'rgba(250,249,247,0.92)',
            border: `.5px solid ${dark ? 'rgba(255,255,255,0.08)' : 'rgba(255,255,255,0.6)'}`,
            backdropFilter: 'blur(24px) saturate(160%)',
            boxShadow: '0 12px 40px rgba(0,0,0,0.18)',
            color: dark ? '#F4F1EB' : '#29261b',
          }}
        >
          {/* Header */}
          <div
            className="flex items-center justify-between px-4 py-3 cursor-move select-none"
            onMouseDown={onDragStart}
            style={{ borderBottom: `1px solid ${dark ? 'rgba(255,255,255,0.06)' : 'rgba(0,0,0,0.06)'}` }}
          >
            <span className="text-[12px] font-semibold tracking-[0.01em]">Settings</span>
            <button
              onClick={() => setOpen(false)}
              onMouseDown={e => e.stopPropagation()}
              className="w-[22px] h-[22px] flex items-center justify-center rounded-md text-[13px]"
              style={{ color: dark ? 'rgba(244,241,235,0.5)' : 'rgba(41,38,27,0.5)', background: 'transparent' }}
            >✕</button>
          </div>

          {/* Body */}
          <div className="p-4 space-y-4">
            {/* Appearance section */}
            <p className="text-[10px] font-semibold tracking-[0.06em] uppercase" style={{ color: dark ? 'rgba(244,241,235,0.4)' : 'rgba(41,38,27,0.4)' }}>Appearance</p>

            {/* Dark mode toggle */}
            <div className="flex items-center justify-between">
              <span className="text-[11.5px]">Dark mode</span>
              <button
                onClick={() => setDark(!dark)}
                role="switch"
                aria-checked={dark}
                className="relative w-8 h-[18px] rounded-full border-0 p-0 transition-colors"
                style={{ background: dark ? '#34c759' : 'rgba(0,0,0,0.15)' }}
              >
                <span
                  className="absolute top-[2px] w-[14px] h-[14px] rounded-full transition-transform"
                  style={{
                    left: 2,
                    background: '#fff',
                    boxShadow: '0 1px 2px rgba(0,0,0,0.25)',
                    transform: dark ? 'translateX(14px)' : 'translateX(0)',
                  }}
                />
              </button>
            </div>

            {/* Accent color */}
            <div className="space-y-2">
              <span className="text-[11.5px]">Accent color</span>
              <div className="flex gap-1.5">
                {ACCENT_PRESETS.map(c => (
                  <button
                    key={c}
                    onClick={() => setAccent(c)}
                    className="flex-1 h-[46px] rounded-md transition-transform"
                    style={{
                      background: c,
                      boxShadow: accent === c
                        ? '0 0 0 1.5px rgba(0,0,0,0.85), 0 2px 6px rgba(0,0,0,0.15)'
                        : '0 0 0 .5px rgba(0,0,0,0.12)',
                    }}
                    aria-label={`Accent ${c}`}
                  >
                    {accent === c && (
                      <svg viewBox="0 0 14 14" className="mx-auto" width={13} height={13}>
                        <path d="M3 7.2 5.8 10 11 4.2" fill="none" strokeWidth="2.2"
                          strokeLinecap="round" strokeLinejoin="round"
                          stroke={hexIsLight(c) ? 'rgba(0,0,0,.78)' : '#fff'} />
                      </svg>
                    )}
                  </button>
                ))}
              </div>
            </div>

            {/* Density */}
            <div className="space-y-2">
              <span className="text-[11.5px]">Density</span>
              <div className="flex rounded-lg overflow-hidden" style={{ background: 'rgba(0,0,0,0.06)', padding: 2, gap: 0 }}>
                {(['compact', 'regular', 'comfy'] as const).map(d => (
                  <button key={d}
                    onClick={() => setDensity(d)}
                    className="flex-1 py-1 text-[11px] font-medium rounded-md transition-colors capitalize"
                    style={{
                      background: density === d ? (dark ? 'rgba(255,255,255,0.1)' : 'rgba(255,255,255,0.9)') : 'transparent',
                      boxShadow: density === d ? '0 1px 2px rgba(0,0,0,0.12)' : 'none',
                    }}>
                    {d}
                  </button>
                ))}
              </div>
            </div>

            {/* Narrative section */}
            <p className="text-[10px] font-semibold tracking-[0.06em] uppercase pt-2" style={{ color: dark ? 'rgba(244,241,235,0.4)' : 'rgba(41,38,27,0.4)' }}>Narrative</p>

            {/* Story tone */}
            <div className="space-y-2">
              <span className="text-[11.5px]">Story tone</span>
              <div className="flex rounded-lg overflow-hidden" style={{ background: 'rgba(0,0,0,0.06)', padding: 2 }}>
                {(['terse', 'detailed'] as const).map(t => (
                  <button key={t}
                    onClick={() => setTone(t)}
                    className="flex-1 py-1 text-[11px] font-medium rounded-md transition-colors capitalize"
                    style={{
                      background: tone === t ? (dark ? 'rgba(255,255,255,0.1)' : 'rgba(255,255,255,0.9)') : 'transparent',
                      boxShadow: tone === t ? '0 1px 2px rgba(0,0,0,0.12)' : 'none',
                    }}>
                    {t}
                  </button>
                ))}
              </div>
            </div>
          </div>
        </div>
      )}
    </>
  )
}
