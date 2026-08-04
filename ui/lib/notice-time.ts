//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Renders a system notice's `raised_at` as a short "since …" stamp for
// NoticeBar. Exists because the db.corrupt banner outlived its own repair
// twice on 2026-08-04 - a 05:15 fault still showing over a healthy database
// twelve hours later - and nothing on screen said the alarm was stale. The
// repair now clears that notice on success; this stamp is the visible check
// on every notice that remains.
//
// Pure and exported for `__tests__/notice-raised-at.test.ts`.

/** "since 10:45 AM" for a notice raised today, "since Aug 1, 9:05 AM" for an
 * older one, '' when `raisedAt` does not parse (never "Invalid Date" in the
 * banner). `now` is injectable for tests. */
export function formatSince(raisedAt: string, now: Date = new Date()): string {
  const d = new Date(raisedAt)
  if (Number.isNaN(d.getTime())) return ''
  const sameDay =
    d.getFullYear() === now.getFullYear() &&
    d.getMonth() === now.getMonth() &&
    d.getDate() === now.getDate()
  const time = d.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })
  if (sameDay) return `since ${time}`
  const day = d.toLocaleDateString([], { month: 'short', day: 'numeric' })
  return `since ${day}, ${time}`
}
