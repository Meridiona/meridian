//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

import { readFileSync } from 'node:fs'

/** Strip comments from source text.
 *
 *  Several suites here assert on the SHAPE of code rather than rendering it, and
 *  the module headers above the files under test routinely describe the very
 *  behaviour being guarded against - the bug that was removed, the optimistic
 *  update that no longer happens. Scanning raw text conflates documenting an old
 *  bug with still doing it, so comments come out first. */
export function stripComments(src: string): string {
  return src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/^\s*\/\/.*$/gm, '')
}

/** Read a file under `ui/` and strip its comments, in one step.
 *
 *  `rel` is relative to the `ui/` root and must start with a slash, e.g.
 *  `/app/setup/page.tsx`. */
export function sourceOf(uiRoot: string, rel: string): string {
  return stripComments(readFileSync(uiRoot + rel, 'utf8'))
}
