//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// Wizard step bodies + Welcome + the STEPS meta (ported from the design's
// steps.jsx). Every body is driven by the live `Wiz` handle built in
// page.tsx — permissions, integrations, sign-in, and the AI-provider choice are
// all real. Nothing here fabricates data.

import type { ReactNode } from 'react'
import { Btn, Check, DISPLAY, Logo, PermIcon, PermissionPreview, Row } from './atoms'
import { PERMISSIONS } from './data'
import type { NotifState, PermissionMeta } from './data'
import type { IntegrationsResponse } from '@/lib/api-types'
import { TRACKERS } from '@/lib/integrations'
import { llmProvider, LLM_INTRO_BODY, LLM_INTRO_TITLE, type LlmProviderId } from '@/lib/llm-providers'
import ConnectTrackers from '@/components/IntegrationConnect'
import LlmProviderPicker, { type InstallOutcome, type ProviderStatus } from '@/components/LlmProviderPicker'
import { SignInWidget } from './signin'

/** The live wizard handle page.tsx builds and threads to every step body. */
export interface Wiz {
  // The resolved OS (`get_platform`). Shapes the Permissions step: macOS shows
  // the two TCC grants + notifications; Windows shows notifications only (see
  // the step-meta note below and `buildSteps`). `null` until resolved.
  platform: string | null
  // Step 1 — permissions (live, polled every 2 s). The two TCC grants are
  // booleans; notifications is tri-state (see `NotifState`) because deny and
  // not-yet-asked need different grant actions.
  perms: { accessibility: boolean | null; screen: boolean | null; notifications: NotifState | null }
  openPane: (pane: string) => void
  grantScreen: () => void
  grantNotifications: (alreadyDenied: boolean) => void
  // Step 2 — integrations (live connected-state from get_integrations)
  integrations: IntegrationsResponse | null
  refetchIntegrations: () => void
  // Step 3 — sign in (Clerk email one-time-code — see ui/app/setup/signin.tsx).
  // The step body owns its own form/busy/error state; `onSignedIn` is just
  // how it reports a completed sign-in back up to the wizard.
  signedInEmail: string | null
  onSignedIn: (email: string) => void
  // Step 4 — intelligence. The one AI choice everything downstream obeys
  // (settings.json `llm_provider` → src/llm/resolver.rs). `providers` is the live
  // which-CLIs-are-installed probe; it never gates the choice, only informs it.
  provider: LlmProviderId
  /** Which custom endpoint, when `provider` is 'custom'. */
  providerCustomId: string | null
  /** Resolves when the choice is persisted, rejects if it wasn't - the shared detail view
   *  awaits this to render "Switching…" and surface a failure. */
  setProvider: (id: LlmProviderId, customId?: string) => Promise<void>
  providers: Record<string, ProviderStatus>
  scanningProviders: boolean
  testingProviderIds: Set<string>
  installingProviderIds: Set<string>
  signingProviderIds: Set<string>
  testProvider: (id: string) => void
  installProvider: (id: string) => Promise<InstallOutcome>
  signInProvider: (id: string) => Promise<InstallOutcome>
  rescanProviders: () => void
}

// ── STEP 1 — Permissions ──────────────────────────────────────────────────────
// One card per OS permission. macOS shows all three (Accessibility + Screen
// Recording required, Notifications optional); Windows shows Notifications
// alone — the two TCC grants have no consent analogue there (see the step-meta
// note below), so their cards would be permanent always-green no-ops.
function PermissionsBody({ wiz }: { wiz: Wiz }) {
  const isWin = wiz.platform === 'windows'
  // Only Screen Recording lives locally on the Mac; on Windows the same
  // reassurance holds, just for "this PC".
  const device = isWin ? 'PC' : 'Mac'
  // On Windows the notifications card IS the whole step, so when it hides —
  // no plugin (`unbundled tauri dev`) or a WinRT probe that returned None in a
  // packaged build — the step would otherwise render blank. Show a fallback
  // line so there is always something to read. (On macOS the two TCC cards
  // below are still there, so no fallback is needed there.)
  const notifHidden = isWin && wiz.perms.notifications === 'unavailable'
  const notifCard = PERMISSIONS.find((p) => p.id === 'notifications')!
  const requiredCards = PERMISSIONS.filter((p) => p.id !== 'notifications')
  // Which card is "up next". Three equally-weighted cards, each with its own
  // Open Settings button, gave no sense of order - so people opened all three
  // panes at once, lost track of which they had actually granted, and came
  // back to a step that looked identical to when they left. One ring at a
  // time turns it into a sequence.
  //
  // The ring only MARKS the next one; every card stays clickable. Gating the
  // later two behind the earlier ones was the other option and is a trap:
  // macOS grants routinely need a relaunch before the probe sees them, so a
  // lagging read would lock the user out of the rest of the step with nothing
  // to click.
  const currentId = [...requiredCards, notifCard].find((p) => !permGranted(p, wiz) && !permHidden(p, wiz))?.id ?? null
  return (
    // Sized to its own content (no flex:1 stretch) - this step's cards are
    // short, and forcing them to fill the whole card height (as a prior pass
    // did) just left a big empty gap inside each one. Scoped to this step
    // only: the shared 628 card height, and every other step's own layout,
    // is untouched.
    <div className="flex flex-col" style={{ gap: 12 }}>
      <span className="flex items-center shrink-0" style={{
        gap: 5, alignSelf: 'flex-start', borderRadius: 99, padding: '4px 10px',
        background: 'color-mix(in srgb, var(--color-state-approved) 10%, transparent)',
        border: '0.5px solid color-mix(in srgb, var(--color-state-approved) 30%, transparent)',
      }}>
        <PermIcon icon="shield" size={11} />
        <span className="mt-chip" style={{ color: 'var(--color-state-approved)', letterSpacing: '.06em' }}>Privacy first</span>
      </span>

      {/* All three permissions in one row, not 2+1 - Notifications joins
         Accessibility/Screen Recording as an equal third column instead of a
         separate full-width row below them. */}
      {!isWin && (
        <div className="flex items-stretch" style={{ gap: 12 }}>
          {requiredCards.map((p) => <PermCard key={p.id} p={p} wiz={wiz} current={p.id === currentId} />)}
          <PermCard p={notifCard} wiz={wiz} current={notifCard.id === currentId} />
        </div>
      )}
      {/* Windows only ever shows Notifications (no TCC analogue for the other
         two there) - it's the whole step, so it renders alone rather than in
         the three-column row above. */}
      {isWin && (
        notifHidden ? (
          <Row tone="surface">
            <span className="flex items-center justify-center shrink-0" style={{
              width: 34, height: 34, borderRadius: 10,
              background: 'var(--t-box)', color: 'var(--t-faint)', border: '0.5px solid var(--t-card-border)',
            }}><PermIcon icon="bell" /></span>
            <div style={{ flex: 1, minWidth: 0 }}>
              <span style={{ fontSize: 13.5, fontWeight: 500, color: 'var(--t-title)' }}>Notifications</span>
              <p style={{ fontSize: 11.5, lineHeight: 1.4, color: 'var(--t-muted)', marginTop: 3 }}>Notification settings aren&apos;t available in this build - you can turn them on later from Windows Settings.</p>
            </div>
          </Row>
        ) : (
          <PermCard p={notifCard} wiz={wiz} current={notifCard.id === currentId} />
        )
      )}
      <p className="flex items-start" style={{ gap: 7, fontSize: 11, lineHeight: 1.5, color: 'var(--t-muted)', marginTop: 3 }}>
        <span style={{ width: 5, height: 5, borderRadius: 99, background: 'var(--color-state-approved)', marginTop: 5, flexShrink: 0 }} />
        Read on this {device}. Never stored, never collected by Meridian.
      </p>
    </div>
  )
}

// A single OS-permission card, shared by macOS and Windows. Notifications is
// tri-state (deny and not-yet-asked need different grant actions); the two TCC
// grants are booleans. Vertical layout (icon+badge, then title/desc, then an
// OS-styled preview of the toggle itself, then the grant action) rather
// than the old single horizontal bar — the two required cards render side by
// side (see PermissionsBody) and stretch to fill the step, and a vertical
// card is what gives the preview room to grow into that height instead of
// squeezing everything onto one row.
/** Whether `p` has been granted.
 *
 *  Extracted so [`PermissionsBody`]'s "which card is next" walk and the card's
 *  own styling read the same predicate. Two copies of this would drift, and
 *  the failure is silent and confusing: a ring sitting on an already-green
 *  card while the genuinely pending one looks finished. */
function permGranted(p: PermissionMeta, wiz: Wiz): boolean {
  return p.id === 'notifications' ? wiz.perms.notifications === 'granted' : !!wiz.perms[p.id]
}

/** Whether `p` renders no card at all - an unbundled run (`tauri dev`) has no
 *  notification plugin, so there is nothing to grant. Kept next to
 *  [`permGranted`] because the "next card" walk must skip these: pointing the
 *  ring at a card that does not exist stalls the sequence on an empty slot. */
function permHidden(p: PermissionMeta, wiz: Wiz): boolean {
  return p.id === 'notifications' && wiz.perms.notifications === 'unavailable'
}

function PermCard({ p, wiz, current }: { p: PermissionMeta; wiz: Wiz; current: boolean }) {
  const notif = p.id === 'notifications'
  // Unbundled runs (`tauri dev`) have no notification plugin at all — nothing
  // to grant, so the card hides rather than dead-ends.
  if (permHidden(p, wiz)) return null
  const granted = permGranted(p, wiz)
  const isMac = wiz.platform !== 'windows'
  // The reconstructed macOS pane already carries its own title + description,
  // lifted from the real System Settings copy - repeating our own title/desc
  // above it just said the same thing twice, so every macOS permission skips
  // the text (not the card itself - the bordered boundary stays, just without
  // the duplicate copy inside it). Windows' Notifications preview is a bare
  // toggle row with no such built-in copy, so it keeps its own title/desc.
  const selfDescribing = isMac
  // One badge, one state: REQUIRED/OPTIONAL until granted, then it flips to
  // GRANTED in place - rather than the badge staying "REQUIRED" forever while
  // a separate "Granted" line appeared below. Every permission here is
  // required now (including Notifications - see PERMISSIONS in data.ts), but
  // the fallback stays in case that ever changes again.
  const badge = granted
    ? { label: 'GRANTED', color: 'var(--color-state-approved)', border: 'var(--color-state-approved)' }
    : { label: p.required ? 'REQUIRED' : 'OPTIONAL', color: 'var(--t-muted)', border: 'var(--t-card-border)' }
  const badgeEl = (
    <span className="mt-chip" style={{ color: badge.color, border: `0.5px solid ${badge.border}`, borderRadius: 4, padding: '1px 5px' }}>{badge.label}</span>
  )
  return (
    <div className="flex flex-col" style={{
      flex: 1, minWidth: 0, gap: selfDescribing ? 6 : 8, padding: '11px 13px', borderRadius: 13,
      // Granted turns the whole card green - a clear at-a-glance "this one's
      // done", not just a faint accent tint (which used to still read as
      // pending at a glance).
      border: `0.5px solid ${granted ? 'color-mix(in srgb, var(--color-state-approved) 40%, var(--t-card-border))' : current ? 'var(--t-accent)' : 'var(--t-card-border)'}`,
      background: granted ? 'color-mix(in srgb, var(--color-state-approved) 9%, var(--t-card))' : 'var(--t-card)',
      // The ring that says "this one next". A box-shadow, not a thicker
      // border, so turning it on cannot change the card's box size and shunt
      // the other two sideways as the sequence advances.
      boxShadow: current ? '0 0 0 2px color-mix(in srgb, var(--t-accent) 32%, transparent)' : undefined,
    }}>
      {selfDescribing ? (
        <div className="flex justify-end">{badgeEl}</div>
      ) : (
        <>
          <div className="flex items-center justify-between">
            <span className="flex items-center justify-center shrink-0" style={{
              width: 34, height: 34, borderRadius: 10,
              background: granted ? 'color-mix(in srgb, var(--color-state-approved) 14%, transparent)' : 'var(--t-box)',
              color: granted ? 'var(--color-state-approved)' : 'var(--t-faint)',
              border: '0.5px solid var(--t-card-border)',
            }}><PermIcon icon={p.icon} /></span>
            {badgeEl}
          </div>
          <div>
            <span style={{ fontSize: 13.5, fontWeight: 500, color: 'var(--t-title)' }}>{p.name}</span>
            <p style={{ fontSize: 11.5, lineHeight: 1.4, color: 'var(--t-muted)', marginTop: 3 }}>{p.desc}</p>
          </div>
        </>
      )}

      {/* Directly under the pane preview, full-width, and clicking it goes
         straight to that permission's own System Settings pane (`openPane`
         is keyed per-permission - screen_recording/accessibility/notifications,
         never a generic prefs URL). Once granted the badge above already says
         so, so this slot clears instead of repeating it here. */}
      <div className="flex items-center" style={{ flex: 1 }}>
        <PermissionPreview os={isMac ? 'macos' : 'windows'} permissionId={p.id} />
      </div>

      {!granted && (
        <div style={{ paddingTop: 2 }}>
          {notif
            // 'prompt' → the button surfaces the one-shot OS dialog (macOS only —
            // Windows reports only granted/denied, never prompt);
            // 'denied' → the OS won't re-prompt, so it opens the Notifications
            // pane directly (grantNotifications skips the pointless re-request
            // when told it's already denied).
            // The current card's action is the solid variant, the rest stay
            // secondary - so the sequence reads off the buttons themselves and
            // not just the ring around the card. Still only styling: every
            // button here is live whether or not it is the current one.
            ? <Btn size="sm" variant={current ? 'primary' : 'secondary'} onClick={() => wiz.grantNotifications(wiz.perms.notifications === 'denied')} style={{ width: '100%' }}>{wiz.perms.notifications === 'denied' ? 'Open Settings' : 'Allow'}</Btn>
            : <Btn size="sm" variant={current ? 'primary' : 'secondary'} onClick={() => p.id === 'screen' ? wiz.grantScreen() : wiz.openPane(p.pane)} style={{ width: '100%' }}>Open Settings</Btn>}
        </div>
      )}
    </div>
  )
}

/** Rail/summary label for the optional Notifications card on Windows, where it
 *  is the whole permissions step. */
function notifSummary(n: NotifState | null): string {
  return n === 'granted' ? 'Enabled' : n === 'denied' ? 'Off' : 'Optional'
}

// ── STEP 2 — Integrations ─────────────────────────────────────────────────────
// The whole connect surface is the shared <ConnectTrackers> (same component the
// dashboard uses), so all 5 providers + every connect flow live in one place.
function IntegrationsBody({ wiz }: { wiz: Wiz }) {
  const connected = TRACKERS.filter((t) => wiz.integrations?.[t.id]).length
  return (
    <div className="flex flex-col" style={{ gap: 9 }}>
      <ConnectTrackers integrations={wiz.integrations} onChanged={wiz.refetchIntegrations} compact />
      <p className="flex items-start" style={{ gap: 7, fontSize: 11, lineHeight: 1.5, color: 'var(--t-muted)', marginTop: 3 }}>
        <span style={{ width: 5, height: 5, borderRadius: 99, background: connected ? 'var(--color-state-approved)' : 'var(--t-faint-2)', marginTop: 5, flexShrink: 0 }} />
        {connected > 0
          ? `${connected} connected · Meridian will match sessions and draft worklogs.`
          : 'Optional - skip it and Meridian still tracks your day. Connect a tracker anytime from Settings to auto-draft worklogs.'}
      </p>
    </div>
  )
}

// ── STEP 3 — Sign in ──────────────────────────────────────────────────────────
// Clerk email one-time-code, entirely inside the webview via tauri-plugin-clerk
// (see lib.rs's plugin registration + signin.tsx). <SignInWidget> is a thin
// next/dynamic boundary around the actual Clerk-aware form (the signin/ module —
// see SignInWidget.tsx/EmailCodeForm.tsx) — importing it here stays safe for the
// static-export build because signin.tsx itself has no @clerk/react imports at
// the top level.
function SignInBody({ wiz }: { wiz: Wiz }) {
  if (wiz.signedInEmail) {
    return (
      <div className="flex flex-col items-center" style={{ width: '100%', maxWidth: 340, margin: '0 auto' }}>
        <div className="w-full flex flex-col items-center mer-pop" style={{
          gap: 10, borderRadius: 16, padding: '26px 26px 22px', textAlign: 'center',
          border: '0.5px solid var(--t-card-border)',
          background: 'color-mix(in srgb, var(--t-accent) 6%, var(--t-card))',
        }}>
          <span className="flex items-center justify-center shrink-0" style={{
            width: 42, height: 42, borderRadius: 13,
            background: 'color-mix(in srgb, var(--t-accent) 14%, transparent)',
            color: 'var(--t-accent)',
          }}><Check size={19} color="var(--t-accent)" w={2.2} /></span>
          <div>
            <p style={{ fontSize: 14.5, fontWeight: 600, color: 'var(--t-title)' }}>You&apos;re signed in</p>
            <p style={{ fontSize: 12, color: 'var(--t-muted)', marginTop: 3 }}>{wiz.signedInEmail}</p>
          </div>
        </div>
      </div>
    )
  }
  return <SignInWidget onSignedIn={wiz.onSignedIn} />
}

// ── STEP 4 — Intelligence (which AI writes the summaries) ────────────────────
// One choice, one place. Everything downstream reads it from settings.json — the
// resolver re-reads it on every call (src/llm/resolver.rs), so changing it later in
// Settings takes effect on the next hour with nothing to restart.
//
// The whole surface is the shared <LlmProviderPicker>: a chooser of the three recommended
// coding agents (+ bring-your-own-key), each opening a detail view that installs the CLI,
// confirms the sign-in, and sets the provider as default. The choice is never blocked on the
// CLI being installed — the detail view walks the user through getting it there instead.
function IntelligenceBody({ wiz }: { wiz: Wiz }) {
  return (
    <LlmProviderPicker
      // First run: ask about a subscription before showing any provider. Without it this
      // step opens on three CLI names and an "advanced" tile, which is unanswerable for
      // anyone who does not already pay for one of them - and they are the users most
      // likely to be here.
      gate
      value={wiz.provider}
      selectedCustomId={wiz.providerCustomId}
      onChange={wiz.setProvider}
      status={wiz.providers}
      scanning={wiz.scanningProviders}
      testingIds={wiz.testingProviderIds}
      installingIds={wiz.installingProviderIds}
      signingIds={wiz.signingProviderIds}
      testOne={wiz.testProvider}
      install={wiz.installProvider}
      signIn={wiz.signInProvider}
      rescan={wiz.rescanProviders}
    />
  )
}

// ── Welcome (pre-step intro) ──────────────────────────────────────────────────
export function Welcome({ onBegin, ready, error, onRetry }: {
  onBegin: () => void
  /** False until `get_platform` resolves — "Get started" stays disabled so the
   *  step list (which depends on platform) can't reshape after the user begins. */
  ready: boolean
  error: boolean
  onRetry: () => void
}) {
  // Three short benefit clauses, not feature labels - one voice, one payoff
  // each, no tracker brand names (those mean nothing until step 2).
  const pillars = [
    { icon: 'shield', label: 'Your screen never leaves this device' },
    { icon: 'power', label: 'Understands your day automatically' },
    { icon: 'mail', label: 'Drafts updates, ready to post' },
  ]
  // THIS SCREEN HAD EVERY GENERATED-PAGE TELL AT ONCE, and they compounded: a
  // pink-to-violet-to-indigo gradient badge (that exact ramp is the signature of
  // an unedited AI export, and it was hardcoded hex - outside the token system
  // entirely), a headline split across two <h1>s with the second one coloured in
  // the accent, an exclamation mark, three radius-99 pills each carrying a filled
  // circular icon, and a segmented chip inside the primary button. Five different
  // devices, all shouting, none of them hierarchy.
  //
  // The fix is subtraction, not restyling. Size and weight carry the hierarchy;
  // the accent appears ONCE, on the one thing that is clickable. What is left is
  // a masthead, a sentence, what the product does, and the way forward - which is
  // all this screen ever had to say.
  return (
    // Full-width flow, vertically centred as ONE block, and left-aligned all the
    // way down: every line starts on the same rail, so the eye tracks straight
    // down it instead of re-finding the start of each block. The founding-user
    // mark shares the brand row (same row, top-right) rather than standing as a
    // column of its own, which used to race the pillars for height and leave a
    // hole in the layout.
    <div className="flex flex-col justify-center" style={{ height: '100%', padding: '0 64px' }}>
      <div>
        <div className="flex items-baseline justify-between" style={{ gap: 24, marginBottom: 30 }}>
          <div className="flex items-center mer-pop" style={{ gap: 13 }}>
            <Logo size={40} />
            <span style={{ fontFamily: 'var(--font-sans)', fontWeight: 700, fontSize: 29, lineHeight: 1, letterSpacing: '-.02em', color: 'var(--t-title)' }}>Meridian</span>
          </div>

          {/* A MARK, not a numbered edition. This carried "No. 042" beside it,
             which read as a real fact about the person holding the screen - their
             place in the queue - and was not one: the number was a hand-edited
             constant, so every user was told they were the 42nd. A number that
             specific is exactly the kind a reader checks the rest of the product
             against, so a false one costs more than it ever bought. What is left
             is true of everyone who sees it. No fill, no shadow, no radius; a
             hairline on the left ties it to the brand row without drawing a box
             around it. */}
          <div className="mer-pop shrink-0 flex items-baseline" style={{
            paddingLeft: 16, borderLeft: '1px solid var(--t-hair)',
          }}>
            <span className="mt-chip" style={{ color: 'var(--t-faint)', letterSpacing: '.08em' }}>Founding user</span>
          </div>
        </div>

        {/* ONE headline. It was two, at the same size, the second one in the
            accent - which is not hierarchy, it is two titles fighting. The
            greeting is a greeting, so it is set as one, above and small; the
            promise is the thing being made, so it is the heading.

            IN THE LAVENDER, WHOLE. Not the button's fill itself - that colour on
            white is ~1.9:1, a surface tone that turns to a smear when it is set
            as type - but the same hue carried down the ramp to where it clears
            the large-text contrast line (`--lavender-ink`). And the WHOLE
            sentence, not the second half: which words land on line two depends
            on the window width, so colouring "work go unnoticed" would mean a
            colour change that moves mid-phrase whenever the text rewraps. */}
        <p style={{ fontSize: 14, fontWeight: 500, color: 'var(--t-muted)', marginBottom: 10 }}>
          You made it here.
        </p>
        <h1 style={{ ...DISPLAY, fontSize: 40, lineHeight: 1.14, color: 'var(--lavender-ink)', textWrap: 'balance', maxWidth: 620 }}>
          We won&apos;t let your work go unnoticed.
        </h1>

        <p style={{ fontSize: 14, lineHeight: 1.65, color: 'var(--t-muted)', marginTop: 16, maxWidth: 560, textWrap: 'pretty' }}>
          You&apos;re one of the first people helping us build Meridian. We hope it
          makes a real difference for you, too.
        </p>

        {/* WHAT IT DOES, as three plain lines. They were chips - filled circular
            icons on radius-99 capsules, three across - which is the shape of a
            feature grid on a landing page, and it made three quiet facts look
            like an advert. Set as text with the icons at the same weight as the
            words beside them, the eye reads them instead of counting them. */}
        <ul className="flex flex-col" style={{ gap: 9, marginTop: 26 }}>
          {pillars.map((p) => (
            <li key={p.label} className="flex items-center" style={{
              gap: 10, fontSize: 13.5, color: 'var(--t-title)',
            }}>
              <span className="shrink-0 flex items-center" style={{ color: 'var(--t-faint)' }}>
                <PermIcon icon={p.icon} size={15} />
              </span>
              {p.label}
            </li>
          ))}
        </ul>
      </div>

      <div style={{ borderTop: '1px solid var(--t-hair)', marginTop: 34, paddingTop: 22 }}>
        {error ? (
          <div className="flex items-center" style={{ gap: 14 }}>
            <p style={{ fontSize: 12, color: 'var(--color-state-pending)' }}>Couldn&apos;t detect your platform - try again.</p>
            <Btn onClick={onRetry} style={{ padding: '12px 28px', fontSize: 14 }}>Retry</Btn>
          </div>
        ) : (
          <div className="flex items-center justify-between" style={{ gap: 24 }}>
            {/* The time estimate belongs to this sentence, not inside the button.
                It was a divided chip within the label - a control carrying two
                pieces of text separated by a rule, which reads as two controls
                and makes the one you press harder to name. */}
            <p style={{ fontSize: 13, color: 'var(--t-muted)' }}>
              Next up: getting you set up. <span style={{ color: 'var(--t-faint)' }}>About 2 minutes.</span>
            </p>
            <Btn onClick={onBegin} disabled={!ready} style={{ padding: '13px 24px', fontSize: 14 }}>
              {ready ? "Let's get started" : 'Preparing setup…'}
            </Btn>
          </div>
        )}
      </div>
    </div>
  )
}

// ── STEP META — order, labels, headers, gating, rail status ───────────────────
export interface StepMeta {
  id: string
  n: string
  label: string
  kicker: string
  title: string
  subtitle: string
  Body: (props: { wiz: Wiz }) => ReactNode
  status: (w: Wiz) => string
  canNext: (w: Wiz) => boolean
}

// Sign in comes first (so the wizard knows who it's setting up for from the
// start), then Permissions (capture needs the grants on macOS); the
// AI-provider choice stays last so the user has connected their trackers and
// signed in before picking which model writes their summaries.
//
// The step is platform-SHAPED, not platform-dropped. macOS capture needs
// Accessibility + Screen Recording — TCC grants with no analogue on Windows,
// where UIA + WGC capture needs no consent (screenpipe_a11y::platform::windows
// reports every one as already true) — so those two cards, and the "Two macOS
// permissions…" copy, are macOS-only. Notifications, though, is a real per-app
// setting on BOTH OSes (macOS UNUserNotificationCenter, Windows WinRT
// ToastNotifier::Setting), so Windows keeps a notifications-only variant of
// this step (`WINDOWS_NOTIFICATIONS_STEP`) rather than losing notification
// onboarding entirely. Both variants keep id:'permissions' so page.tsx's
// check_notifications poll fires on either.
const PERMISSIONS_STEP: StepMeta = {
  id: 'permissions', n: '01', label: 'Permissions', kicker: 'Access',
  title: 'Help Meridian reconstruct your day',
  subtitle: "That's what these permissions are for - what Meridian captures stays on this device and is never uploaded.",
  Body: PermissionsBody,
  status: (s) => { const g = [s.perms.accessibility, s.perms.screen].filter(Boolean).length; return g ? `${g} granted` : 'Not granted' },
  canNext: (s) => !!(s.perms.accessibility && s.perms.screen),
}

// Windows variant: notifications is the only user-settable permission there and
// it's optional (on by default for most apps), so this never gates Continue.
// The card, grant action, and deny→Settings recovery are all shared with macOS
// via PermCard — only the copy and the never-blocking gate differ.
const WINDOWS_NOTIFICATIONS_STEP: StepMeta = {
  id: 'permissions', n: '01', label: 'Notifications', kicker: 'Alerts',
  title: 'Turn on notifications',
  // Carries the WHY; the card below carries the WHAT (which events, quiet-hours)
  // so the user doesn't read the same sentence twice in one viewport.
  subtitle: 'Stay in the loop without watching the menu bar. Optional - turn it on now, or anytime from Settings.',
  Body: PermissionsBody,
  status: (s) => notifSummary(s.perms.notifications),
  canNext: () => true,
}

const SIGNIN_STEP: StepMeta = {
  id: 'signin', n: '01', label: 'Sign in', kicker: 'Account',
  title: 'Sign in to Meridian',
  subtitle: "So we know who's using Meridian.",
  Body: SignInBody,
  status: (s) => s.signedInEmail ?? 'Not signed in',
  canNext: (s) => !!s.signedInEmail,
}

// ── Steps the WIZARD no longer runs — owned by the post-setup walkthrough ─────
// Both of these used to be wizard steps 03/04. They moved because upfront they
// are unmotivated: "connect your Jira" before the user has seen anything reads
// as a data grab, and "pick an AI model" means nothing before they have read
// something a model wrote. The walkthrough asks for each at the moment its value
// just became visible on screen — same ask, very different conversion.
//
// They keep their StepMeta shape (rather than collapsing to bare components) so
// the walkthrough gets the title/subtitle/status/gating copy for free and there
// is still exactly one definition of each. Not in [`ALL_STEPS`] — nothing else
// should add them back without moving the corresponding walkthrough beat.
export const INTEGRATIONS_STEP: StepMeta = {
  id: 'integrations', n: '03', label: 'Integrations', kicker: 'Project tools',
  title: 'Connect your trackers',
  subtitle: 'Link the tools you use so Meridian can match sessions to tickets and draft worklogs - skip it and Meridian still tracks your day; connect anytime from Settings.',
  Body: IntegrationsBody,
  status: (s) => { const c = TRACKERS.filter((t) => s.integrations?.[t.id]).length; return c ? `${c} connected` : 'Optional' },
  canNext: () => true,
}

export const PROVIDER_STEP: StepMeta = {
  id: 'provider', n: '04', label: 'Intelligence', kicker: 'Your AI',
  // The SHARED copy - the same words Settings shows. These used to be hardcoded here with
  // different wording while llm-providers.ts claimed they were shared.
  title: LLM_INTRO_TITLE,
  subtitle: LLM_INTRO_BODY,
  Body: IntelligenceBody,
  status: (s) => llmProvider(s.provider).name,
  // Never gates. Every path out of this step is valid — including picking a CLI that
  // isn't installed yet: that hour is left pending rather than dead-ending setup.
  canNext: () => true,
}

// The wizard proper: identity, then the grants capture cannot run without.
// Everything else the user can decide once they have seen what Meridian does.
const ALL_STEPS: StepMeta[] = [
  SIGNIN_STEP,
  PERMISSIONS_STEP,
]

/** Platform-specific step list. On Windows the Permissions step is swapped for
 *  its notifications-only variant (same id, so the poll still fires); the count
 *  is unchanged. `n` is renumbered to position so the rail never shows a gap. */
export function buildSteps(platform: string | null): StepMeta[] {
  const steps = platform === 'windows'
    ? ALL_STEPS.map((s) => (s.id === 'permissions' ? WINDOWS_NOTIFICATIONS_STEP : s))
    : ALL_STEPS
  return steps.map((s, i) => ({ ...s, n: String(i + 1).padStart(2, '0') }))
}
