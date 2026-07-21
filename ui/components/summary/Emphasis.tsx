//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// `**emphasis**` → an emphasised span. The ONLY markup this screen renders.
//
// The narrative has to be able to lean on the phrase that carried the day - the
// thing that took over, the thing that finally broke open - without turning that
// into a label. An earlier shape gave insights a `kind` enum with `drifted` and
// `overperformed` in it, and a card titled "Drifted" reads as a verdict on the
// person no matter how gently the sentence beneath it is written. Emphasis inside
// the prose says the same thing as an observation rather than a judgement.
//
// Deliberately NOT a markdown renderer. One rule, no library, no dangerouslySet
// HTML: the model writes prose and a full parser would only give it more ways to
// put a heading or a bullet list on a screen that has room for neither.

'use client'

import { Fragment } from 'react'

/** Split on `**…**`, keeping the delimiters' contents. Odd indices are emphasised.
 *  A trailing unmatched `**` simply never opens a span - the text renders whole.
 *
 *  `[\s\S]` rather than the `s` flag: the tsconfig targets a version that predates
 *  dotAll, and a narrative that wrapped a phrase spanning a line break would
 *  otherwise silently show its asterisks. */
function split(text: string): string[] {
  return text.split(/\*\*([\s\S]+?)\*\*/g)
}

/**
 * Render `text` with `**…**` runs emphasised.
 *
 * The emphasis is weight plus a soft accent underlay rather than a colour swap:
 * recolouring a clause mid-sentence breaks the line into two voices, and the
 * point here is a phrase that carries more weight, not a different speaker.
 */
export function Emphasis({ text }: { text: string }) {
  return (
    <>
      {split(text).map((part, i) =>
        i % 2 === 1 ? (
          <em
            key={i}
            className="not-italic"
            style={{
              fontWeight: 650,
              color: 'var(--t-title)',
              // A wash sitting behind the words rather than a highlighter pen
              // over them - it should read as pressure in the voice.
              background:
                'linear-gradient(to top, color-mix(in srgb, var(--accent) 16%, transparent) 0%, color-mix(in srgb, var(--accent) 16%, transparent) 34%, transparent 34%)',
              borderRadius: 2,
              padding: '0 1px',
            }}
          >
            {part}
          </em>
        ) : (
          <Fragment key={i}>{part}</Fragment>
        ),
      )}
    </>
  )
}
