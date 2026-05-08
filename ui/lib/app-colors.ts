// Curated warm 12-color palette — [hue, saturation%, lightness%]
const PALETTE: [number, number, number][] = [
  [220, 55, 58],
  [25,  58, 54],
  [165, 45, 48],
  [280, 40, 55],
  [350, 52, 57],
  [195, 50, 50],
  [42,  60, 52],
  [260, 38, 52],
  [140, 42, 47],
  [10,  55, 56],
  [200, 48, 52],
  [85,  45, 50],
]

function hashString(str: string): number {
  let h = 0
  for (let i = 0; i < str.length; i++) {
    h = (Math.imul(31, h) + str.charCodeAt(i)) | 0
  }
  return Math.abs(h)
}

export function getAppColor(appName: string): string {
  if (appName === '(idle)' || appName === '(away)') return '#C8C6C1'
  const [h, s, l] = PALETTE[hashString(appName) % PALETTE.length]
  return `hsl(${h}, ${s}%, ${l}%)`
}

export function getAppColorBg(appName: string): string {
  if (appName === '(idle)' || appName === '(away)') return '#EDEBE6'
  const [h, s, l] = PALETTE[hashString(appName) % PALETTE.length]
  return `hsl(${h}, ${Math.max(s - 15, 10)}%, ${Math.min(l + 28, 92)}%)`
}

export function getAppInitial(appName: string): string {
  if (appName === '(idle)' || appName === '(away)') return '—'
  return appName.trim().charAt(0).toUpperCase()
}
