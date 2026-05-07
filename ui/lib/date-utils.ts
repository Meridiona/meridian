export function localDayBounds(dateStr: string): { start: string; end: string } {
  const localStart = new Date(`${dateStr}T00:00:00`)
  const localEnd = new Date(`${dateStr}T23:59:59.999`)
  return {
    start: localStart.toISOString(),
    end: localEnd.toISOString(),
  }
}

export function todayString(): string {
  const d = new Date()
  const y = d.getFullYear()
  const m = String(d.getMonth() + 1).padStart(2, '0')
  const day = String(d.getDate()).padStart(2, '0')
  return `${y}-${m}-${day}`
}
