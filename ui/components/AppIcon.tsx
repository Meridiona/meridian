// meridian — AI activity intelligence by Meridiona

import { getAppColor, getAppInitial } from '@/lib/app-colors'

interface AppIconProps {
  appName: string
  size?: 'sm' | 'md' | 'lg'
}

const sizeMap = {
  sm: { container: 'w-6 h-6 text-[10px]' },
  md: { container: 'w-8 h-8 text-xs' },
  lg: { container: 'w-10 h-10 text-sm' },
}

export default function AppIcon({ appName, size = 'md' }: AppIconProps) {
  const color = getAppColor(appName)
  const initial = getAppInitial(appName)
  const { container } = sizeMap[size]

  return (
    <div
      className={`${container} rounded-lg flex items-center justify-center font-semibold text-white shrink-0 select-none`}
      style={{ backgroundColor: color }}
      aria-hidden
    >
      {initial}
    </div>
  )
}
