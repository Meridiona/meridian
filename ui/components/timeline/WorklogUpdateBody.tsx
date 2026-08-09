//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// The generated update itself: the block of text that lands on the ticket.
//
// Split out of `WorklogTargets.tsx` when that file crossed the 500-line rule.
// The seam is the one the module already had: that file answers WHERE the
// worklog goes (matched tickets, a proposed one, the board it lands on) and
// this answers WHAT goes there. `DraftDocument` still composes the two into one
// card, which is the shape the user reads.
//
// Presentation only - nothing here writes.
//
// # Who calls this
// [`DraftDocument`] and [`TargetRow`], both in `./WorklogTargets.tsx`.
//
// # Related
// - `./WorklogTargets.tsx` — the destinations, and the card that frames both
// - `./WorklogDraftDialog.tsx` — the surface all of it is read on

'use client'

import { useState } from 'react'
import type { GeneratedWorklogUpdate } from '@/lib/api-types'
import { Bullets, Field } from './dayTaskKit'

/** Whether a status is short enough to wear the chip it was designed for.
 *
 *  Exported for its unit test - the repo has no React render harness, and the
 *  boundary is the whole point of the rule. */
export function statusIsChipSized(status: string): boolean {
  const s = status.trim()
  return s.length <= 28 && !s.includes('.')
}

/** A model-written heading as a heading, not a shout.
 *
 *  The generator returns these in whatever case it likes - "APPLICATION
 *  SUBMITTED", "Selection process" - and the old renderer forced them all
 *  uppercase, which is what put a section heading in the same clothes as the
 *  card's own label. Only fully-uppercase input is touched; anything already
 *  written as prose is left exactly as the model wrote it. */
export function sentenceCase(heading: string): string {
  const h = heading.trim()
  if (h !== h.toUpperCase()) return h
  return h.charAt(0) + h.slice(1).toLowerCase()
}

/** One update rendered inline: summary, its labelled sections, and a status line.
 *  The shared shape used both per-ticket (in a target row) and for the whole draft.
 *
 *  `boxed` gives it a surface of its own, titled and with the lead paragraph ruled
 *  off from the sections. WITHOUT IT this was a stack of headings and bullets
 *  running flush into the matched-ticket row above and the provenance line below,
 *  so the one thing on the screen that will actually be posted read as more
 *  commentary about the match - a document with no edges and no name. Left off
 *  inside a target row, where the row is already the frame and a second one would
 *  nest. */
export function UpdateBody({ update, boxed = false, unframed = false }: {
  update: GeneratedWorklogUpdate
  boxed?: boolean
  /** `boxed` typography without the frame: `DraftDocument` supplies the card and
   *  the header, and a second border inside it would nest. */
  unframed?: boolean
}) {
  const sections = update.sections.filter((s) => s.heading.trim() && s.points.some((p) => p.trim()))
  const body = (
    <>
      {update.summary && (
        // THE LEAD, and the only thing in the box set above body size. If a
        // reader takes one thing from the card it should be this sentence.
        <p className="mt-body-sm" style={{
          color: 'var(--t-title)',
          fontSize: boxed ? 14 : 13, lineHeight: boxed ? 1.55 : 1.6,
          fontWeight: boxed ? 500 : 400,
          letterSpacing: boxed ? '-0.005em' : undefined,
        }}>
          {update.summary}
        </p>
      )}
      {sections.length > 0 && (
        <div className={boxed ? 'mt-4 pt-4 border-t' : 'space-y-2.5 mt-2.5'}
          style={boxed ? { borderColor: 'var(--t-hair)' } : undefined}>
          {/* BOXED IS READ, NOT SCANNED, so it does not use the `Field`/`Bullets`
              kit the rest of the panel does. That kit is tuned for metadata beside
              a card - an 11px uppercase kicker in `--t-faint` over 12px `--t-muted`
              lines - and at those sizes and contrasts a paragraph of real prose
              reads as blurred rather than quiet.
              WHAT SEPARATES ONE SECTION FROM THE NEXT IS A RULE, not a gap. With
              only vertical space between them, three sections of two bullets each
              read as one seven-item list with stray bold lines in it - the reader
              has to reconstruct the grouping from spacing alone, which is exactly
              the work a document is supposed to have done for them.
              AND A HEADING OUTRANKS ITS OWN CONTENT. It was set SMALLER and
              LIGHTER than the bullets beneath it (12px `--t-muted` over 13px
              `--t-title`), so it read as a caption trailing off the section
              above rather than the title of the one below. Same size as the body
              now, and heavier: weight is the whole distinction, which keeps the
              block quiet while making its structure unambiguous. */}
          {sections.map((sec, i) => boxed ? (
            <div key={`${sec.heading}-${i}`}
              className={i === 0 ? '' : 'mt-4 pt-4 border-t'}
              style={i === 0 ? undefined : { borderColor: 'var(--t-hair)' }}>
              <p className="mb-2" style={{
                color: 'var(--t-title)', fontSize: 13, fontWeight: 700,
                letterSpacing: '-0.01em', lineHeight: 1.4,
              }}>{sentenceCase(sec.heading)}</p>
              <ul className="space-y-2">
                {sec.points.filter((p) => p.trim()).map((p, j) => (
                  <li key={j} className="flex gap-2.5"
                    style={{ color: 'var(--t-title)', fontSize: 13, lineHeight: 1.6, fontWeight: 400 }}>
                    {/* A drawn dot, not a `·`. The middot sits on the text
                        baseline at a size that vanishes next to 13px prose, so
                        the bullets did not read as a list at all. */}
                    <span aria-hidden className="shrink-0 rounded-full" style={{
                      width: 4, height: 4, marginTop: 8, background: 'var(--t-faint)',
                    }} />
                    <span className="flex-1 min-w-0">{p}</span>
                  </li>
                ))}
              </ul>
            </div>
          ) : (
            <Field key={`${sec.heading}-${i}`} label={sec.heading}>
              <Bullets items={sec.points.filter((p) => p.trim())} size={12} />
            </Field>
          ))}
        </div>
      )}
      {update.status && (
        // A STATE, not another bulleted section. Rendered on its own ruled line
        // so it reads as the verdict on everything above it, which is what a
        // reader of the ticket will take from it.
        //
        // A CHIP ONLY WHEN IT IS CHIP-SIZED. The pill was built for "Shipped"
        // and the model does not always oblige - "Antler application submitted
        // and awaiting decision - other accelerator options still being
        // evaluated" arrived as a full sentence in a green lozenge, which reads
        // as a badge that has burst. Past a few words it is set as text instead,
        // with the label above rather than beside it so the line can wrap.
        statusIsChipSized(update.status) ? (
          <div className={boxed ? 'flex items-center gap-2 mt-3.5 pt-3 border-t' : 'flex items-center gap-2 mt-2.5'}
            style={boxed ? { borderColor: 'var(--t-hair)' } : undefined}>
            <span className="mt-label" style={{ color: boxed ? 'var(--t-faint-2)' : 'var(--t-faint)' }}>STATUS</span>
            <span className="rounded-md px-2 py-0.5" style={{
              fontSize: 11.5, fontWeight: 700, color: 'var(--color-state-approved)',
              background: 'color-mix(in srgb, var(--color-state-approved) 12%, transparent)',
            }}>
              {update.status}
            </span>
          </div>
        ) : (
          <div className={boxed ? 'mt-3.5 pt-3 border-t' : 'mt-2.5'}
            style={boxed ? { borderColor: 'var(--t-hair)' } : undefined}>
            <span className="mt-label block mb-1" style={{ color: boxed ? 'var(--t-faint-2)' : 'var(--t-faint)' }}>STATUS</span>
            <span style={{
              fontSize: 12.5, fontWeight: 600, lineHeight: 1.5,
              color: 'var(--color-state-approved)',
            }}>
              {update.status}
            </span>
          </div>
        )
      )}
    </>
  )

  if (!boxed) return <div className="mt-1.5">{body}</div>
  if (unframed) return body

  return (
    <section className="rounded-xl" style={{
      background: 'var(--t-box)', border: '1px solid var(--t-card-border)',
    }}>
      {/* ONE LINE. It used to carry a second, "Posted on the ticket exactly as
          written here" - written to answer the question a draft raises, "is this
          the thing that gets posted or a note to me about it?". The box answers
          that on its own now that it is titled and framed, and the sentence was
          costing more than it earned: a third grey size stacked above the prose,
          which is exactly what made this corner read as mush. */}
      <header className="flex items-center justify-between gap-3 px-4 py-3 border-b"
        style={{ borderColor: 'var(--t-hair)' }}>
        <p className="mt-label" style={{ color: 'var(--t-muted)' }}>THE UPDATE</p>
        <span className="-mr-1.5"><CopyUpdate update={update} /></span>
      </header>
      <div className="px-4 py-3.5">{body}</div>
    </section>
  )
}

/** The update as plain text, in the order it is rendered.
 *
 *  Exported for its unit test - the shape matters more than it looks, because
 *  this is what lands in a standup message or a PR description, and a version
 *  that dropped the section headings would read as one undifferentiated wall.
 *  Markdown-ish rather than the ticket's own markup: the destination is unknown
 *  (Slack, a commit message, an email), and `##` degrades to something legible
 *  everywhere while a tracker's wiki syntax does not. */
export function updateToText(update: GeneratedWorklogUpdate): string {
  const parts: string[] = []
  if (update.summary.trim()) parts.push(update.summary.trim())
  for (const sec of update.sections) {
    const points = sec.points.filter((p) => p.trim())
    if (!sec.heading.trim() || points.length === 0) continue
    parts.push([`## ${sec.heading.trim()}`, ...points.map((p) => `- ${p.trim()}`)].join('\n'))
  }
  if (update.status.trim()) parts.push(`Status: ${update.status.trim()}`)
  return parts.join('\n\n')
}

/** Copy the whole update to the clipboard.
 *
 *  The draft is often wanted somewhere the tracker is not - pasted into a
 *  standup, a PR description, a message to whoever asked. Without this the only
 *  way out of the box was to select prose that spans headings and bullets by
 *  hand, which picks up the labels as body text.
 *
 *  A QUIET ICON, NOT A BORDERED BUTTON. It sat on the header line as an outlined
 *  pill at the same weight as "THE UPDATE" itself, so a title bar with one
 *  affordance read as a row of two controls - and the louder-looking of the two
 *  was the one that matters least. A secondary action on a document header is
 *  conventionally a ghost icon: present when looked for, invisible when not.
 *
 *  AND A DRAWN ICON, NOT `⧉`. U+29C9 is absent from most UI font stacks, so it
 *  fell back to whatever the system had - a different size and baseline from the
 *  label beside it, which is the specific reason it looked wrong rather than
 *  merely plain. */
export function CopyUpdate({ update }: { update: GeneratedWorklogUpdate }) {
  const [done, setDone] = useState(false)
  const copy = () => {
    navigator.clipboard.writeText(updateToText(update))
      .then(() => {
        setDone(true)
        setTimeout(() => setDone(false), 1800)
      })
      // Silent: a clipboard the browser refused is not something the user can
      // act on, and an error toast over a draft they were reading is worse than
      // the button appearing not to have fired.
      .catch(() => {})
  }
  return (
    <button onClick={copy} title="Copy this update" aria-label="Copy this update"
      className="inline-flex items-center gap-1.5 rounded-md px-1.5 py-1 shrink-0 transition-colors"
      style={{
        fontSize: 11, fontWeight: 600, cursor: 'pointer', letterSpacing: '0.01em',
        color: done ? 'var(--color-state-approved)' : 'var(--t-faint)',
        border: 'none', background: 'transparent',
      }}>
      {done ? <TickIcon /> : <CopyIcon />}
      {done ? 'Copied' : 'Copy'}
    </button>
  )
}

function CopyIcon() {
  return (
    <svg width="12" height="12" viewBox="0 0 16 16" fill="none" aria-hidden>
      <rect x="5.6" y="5.6" width="8.4" height="8.4" rx="2"
        stroke="currentColor" strokeWidth="1.4" />
      <path d="M10.8 3.4A1.8 1.8 0 0 0 9 2H3.8A1.8 1.8 0 0 0 2 3.8V9a1.8 1.8 0 0 0 1.4 1.76"
        stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
    </svg>
  )
}

function TickIcon() {
  return (
    <svg width="12" height="12" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path d="M3 8.5 6.3 11.8 13 5" stroke="currentColor" strokeWidth="1.8"
        strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  )
}
