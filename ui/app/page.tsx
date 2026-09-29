//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import MeridianTimelineShell from '@/components/timeline/MeridianTimelineShell'
import { RequireEmailCapture } from '@/app/setup/signin'

// Email capture is compulsory: RequireEmailCapture hides the whole app behind
// an inline capture screen until an email has ever been saved. There is no
// session and no sign-out, so unlike the old RequireSignIn this never re-locks
// once captured.
export default function Root() {
  return (
    <RequireEmailCapture>
      <MeridianTimelineShell />
    </RequireEmailCapture>
  )
}
