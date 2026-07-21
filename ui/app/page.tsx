//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import MeridianTimelineShell from '@/components/timeline/MeridianTimelineShell'
import { RequireSignIn } from '@/app/setup/signin'

// Sign-in is compulsory: RequireSignIn hides the whole app behind an inline
// sign-in screen until a live Clerk session exists, and re-locks on sign-out.
export default function Root() {
  return (
    <RequireSignIn>
      <MeridianTimelineShell />
    </RequireSignIn>
  )
}
