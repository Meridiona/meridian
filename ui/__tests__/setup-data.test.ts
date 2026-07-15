//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { describe, it, expect } from 'bun:test'
import { MODEL_ID, fmtModelLabel } from '../app/setup/data'

describe('fmtModelLabel', () => {
  it('formats the real MODEL_ID without duplicating the parameter size (the doubled "2B" bug)', () => {
    expect(fmtModelLabel(MODEL_ID)).toBe('Qwen3.5 2B · 4-bit')
  })

  it('strips the org prefix', () => {
    expect(fmtModelLabel('mlx-community/Qwen3.5-2B-OptiQ-4bit')).not.toContain('mlx-community')
  })

  it('drops the quantization scheme name', () => {
    expect(fmtModelLabel('mlx-community/Qwen3.5-2B-OptiQ-4bit')).not.toContain('OptiQ')
  })

  it('handles an id with no org prefix', () => {
    expect(fmtModelLabel('Qwen3.5-2B-OptiQ-4bit')).toBe('Qwen3.5 2B · 4-bit')
  })

  it('handles an id with no bit-width suffix', () => {
    expect(fmtModelLabel('mlx-community/Some-Model')).toBe('Some Model')
  })

  it('accepts a hyphenated bit-width form (e.g. "4-bit")', () => {
    expect(fmtModelLabel('org/Name-8-bit')).toBe('Name · 8-bit')
  })
})
