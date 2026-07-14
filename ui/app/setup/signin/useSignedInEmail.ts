//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

import { useEffect, useRef } from 'react'
import { useUser } from '@clerk/react'

/** Shared "tell the parent once a Clerk session exists" pattern — both the
 *  wizard's sign-in step and Settings' account row need to notice a fresh
 *  sign-in and report the email up exactly once, even across Clerk's own
 *  isLoaded/isSignedIn transitions. A ref (not just a dependency check)
 *  guards the single call. `resetNotified` re-arms it — call after a
 *  sign-out so a subsequent sign-in notifies again. */
export function useSignedInEmail(onSignedIn: (email: string) => void) {
  const { isLoaded, isSignedIn, user } = useUser()
  const notified = useRef(false)
  const email = user?.primaryEmailAddress?.emailAddress

  useEffect(() => {
    if (isLoaded && isSignedIn && email && !notified.current) {
      notified.current = true
      onSignedIn(email)
    }
  }, [isLoaded, isSignedIn, email, onSignedIn])

  return {
    isLoaded,
    isSignedIn,
    email,
    resetNotified: () => { notified.current = false },
  }
}
