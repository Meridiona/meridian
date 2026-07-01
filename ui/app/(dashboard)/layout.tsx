//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// After the one-pager migration only /sessions and /week remain under this
// route group, and they're unreachable from nav (kept for reference, per the
// product decision). No shared shell — the old DashboardShell/Sidebar/CommandBar
// are retired; this is a bare passthrough.

export default function DashboardLayout({ children }: { children: React.ReactNode }) {
  return <>{children}</>
}
