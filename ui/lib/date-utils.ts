export function localDayBounds(dateStr: string): { start: string; end: string } {
  // Keep as local ISO strings — do NOT call toISOString() which shifts to UTC.
  return {
    start: `${dateStr}T00:00:00`,
    end: `${dateStr}T23:59:59.999`,
  }
}

export function todayString(): string {
  const d = new Date()
  const y = d.getFullYear()
  const m = String(d.getMonth() + 1).padStart(2, '0')
  const day = String(d.getDate()).padStart(2, '0')
  return `${y}-${m}-${day}`
}
