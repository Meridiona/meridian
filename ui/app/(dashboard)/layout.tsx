//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import DashboardShell from '@/components/DashboardShell'

export default function DashboardLayout({ children }: { children: React.ReactNode }) {
  return <DashboardShell>{children}</DashboardShell>
}
