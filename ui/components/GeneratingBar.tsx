//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
'use client'

// The one "a model is running right now" treatment, shared by every surface that
// makes an LLM call the user is waiting on (worklog generation, the plan's task
// composer). Extracted from DayTaskDetailPanel when the composer needed the same
// thing - one definition, so the wait always looks and reads the same.
//
// WHY IT SAYS HOW LONG: these calls route to whatever provider the user configured,
// and a Claude/Codex CLI hop is measured in tens of seconds, not milliseconds (the
// plan-task draft benchmarks ~4s; a worklog can run close to a minute). A bare spinner with
// no time expectation reads as "hung" at second fifteen and gets clicked again. The
// honest number is the feature.
//
// It is a STATEMENT, not a progress bar: the 55% fill is fixed and pulsing because we
// genuinely don't know how far along the model is. Faking a creeping percentage would
// be a lie the user can time.

/** Indeterminate progress + a plain time expectation, for an in-flight model call.
 *
 *  @param hue    the surface's accent - pass the AI/proposal hue unless the caller
 *                has its own (a day task's category colour, say).
 *  @param label  what is being made, present tense ("Generating your worklog…").
 *  @param detail one line on what is happening and that they can walk away.
 *  @param note   the time expectation. Defaults to the honest, unhurried version. */
export function GeneratingBar({ hue, label, detail, note = 'this might take a minute or so' }: {
  hue: string
  label: string
  detail: string
  note?: string
}) {
  return (
    <div className="space-y-2" role="status" aria-live="polite">
      <div className="flex items-center justify-between gap-3">
        <p className="mt-body-sm inline-flex items-center gap-2" style={{ color: 'var(--t-title)', fontSize: 12.5, fontWeight: 600 }}>
          <span aria-hidden>✨</span> {label}
        </p>
        <span className="mt-body-sm animate-pulse shrink-0" style={{ color: 'var(--t-faint)', fontSize: 11 }}>{note}</span>
      </div>
      <div className="rounded-full overflow-hidden" style={{ height: 4, background: 'var(--t-hair)' }}>
        <div className="h-full rounded-full animate-pulse" style={{ width: '55%', background: hue }} />
      </div>
      <p className="mt-body-sm" style={{ color: 'var(--t-faint)', fontSize: 11, lineHeight: 1.5 }}>
        {detail}
      </p>
    </div>
  )
}
