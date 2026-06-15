//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

import { accessSync, constants } from 'fs'

/**
 * Candidate paths for the meridian binary, in PREFERENCE order.
 *
 * The native Mach-O binary is listed FIRST on purpose: it has no runtime
 * dependencies, so it runs under launchd's stripped PATH. The
 * `~/.local/bin/meridian` wrapper is a `#!/usr/bin/env node` script, so it only
 * works when `node` is resolvable on PATH — true in a dev shell, but NOT under
 * the launchd-managed dashboard (launchd's PATH lacks Homebrew's bin dir). When
 * the wrapper was probed first, the installed dashboard's task sync failed with
 * `env: node: No such file or directory` while dev worked fine — a dev/prod
 * parity gap. Preferring the native binary closes it: same behaviour in both.
 */
export function meridianCandidates(home: string = process.env.HOME ?? ''): string[] {
  return [
    `${home}/.meridian/app/bin/meridian`, // native, no runtime deps — works under launchd
    '/usr/local/bin/meridian', // native, system-wide install
    `${home}/.local/bin/meridian`, // node wrapper — needs `node` on PATH, so last
  ]
}

/** Default executability probe — X_OK against the real filesystem. */
export function isExecutable(path: string): boolean {
  try {
    accessSync(path, constants.X_OK)
    return true
  } catch {
    return false
  }
}

/**
 * Pick the first executable candidate. Falls back to the first candidate when
 * none are executable, so the caller spawns a real path and surfaces a
 * meaningful ENOENT rather than spawning `undefined`. `probe` is injectable so
 * the selection order can be unit-tested without touching the filesystem.
 */
export function selectMeridianBinary(
  candidates: string[],
  probe: (path: string) => boolean = isExecutable,
): string {
  return candidates.find(probe) ?? candidates[0]
}
