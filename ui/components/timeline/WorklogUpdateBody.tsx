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
        <div className={boxed ? 'space-y-3.5 mt-3.5 pt-3.5 border-t' : 'space-y-2.5 mt-2.5'}
          style={boxed ? { borderColor: 'var(--t-hair)' } : undefined}>
          {/* BOXED IS READ, NOT SCANNED, so it does not use the `Field`/`Bullets`
              kit the rest of the panel does. That kit is tuned for metadata beside
              a card - an 11px uppercase kicker in `--t-faint` over 12px `--t-muted`
              lines - and at those sizes and contrasts a paragraph of real prose
              reads as blurred rather than quiet. This is the text that gets posted
              on someone's ticket, so it is set as body copy: solid `--t-title` at
              13px, and headings that are legible without being louder than the
              sentences under them.
              SENTENCE CASE, NOT A KICKER. The uppercase micro-label was doing
              three unrelated jobs on one card - the card's own title, the
              destination header, and every section heading inside the update -
              so three different levels of the hierarchy wore the same clothes
              and the card read as one flat slab. The kicker is now reserved for
              the card's chrome; a section heading is a heading. */}
          {sections.map((sec, i) => boxed ? (
            <div key={`${sec.heading}-${i}`}>
              <p className="mb-1.5" style={{
                color: 'var(--t-muted)', fontSize: 12, fontWeight: 700, letterSpacing: '-0.005em',
              }}>{sentenceCase(sec.heading)}</p>
              <ul className="space-y-1.5">
                {sec.points.filter((p) => p.trim()).map((p, j) => (
                  <li key={j} className="flex gap-2.5"
                    style={{ color: 'var(--t-title)', fontSize: 13, lineHeight: 1.6, fontWeight: 400 }}>
                    <span aria-hidden className="shrink-0" style={{ color: 'var(--t-faint-2)' }}>·</span>
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
        <CopyUpdate update={update} />
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
 *  hand, which picks up the labels as body text. */
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
    <button onClick={copy} title="Copy this update"
      className="mt-body-sm inline-flex items-center gap-1.5 rounded-lg px-2.5 py-1.5 shrink-0"
      style={{
        fontSize: 11.5, fontWeight: 700, cursor: 'pointer',
        color: done ? 'var(--color-state-approved)' : 'var(--t-muted)',
        border: `1px solid ${done ? 'color-mix(in srgb, var(--color-state-approved) 38%, transparent)' : 'var(--t-hair)'}`,
        background: 'transparent',
      }}>
      <span aria-hidden>{done ? '✓' : '⧉'}</span> {done ? 'Copied' : 'Copy'}
    </button>
  )
}
