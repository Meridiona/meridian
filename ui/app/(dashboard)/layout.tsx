// meridian — normalises screenpipe activity into structured app sessions
import DashboardShell from '@/components/DashboardShell'

export default function DashboardLayout({ children }: { children: React.ReactNode }) {
  return <DashboardShell>{children}</DashboardShell>
}
