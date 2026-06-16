//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

import { accessSync, constants } from 'fs'
import { resolve } from 'path'

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
 *
 * In LOCAL DEVELOPMENT (`NODE_ENV === 'development'`, i.e. `npm run dev`) the
 * repo build is prepended ahead of the installed paths. dev-start.sh runs the
 * daemon as `cargo watch -x 'run --bin meridian'` → `target/debug/meridian`,
 * which is rebuilt on every save and advances `meridian.db` as new migrations
 * land. The installed binary under ~/.meridian is a SEPARATE artifact that does
 * NOT track the repo, so the moment a migration is added the daemon migrates the
 * DB forward while the installed CLI stays behind — and sqlx aborts task-sync
 * with the opaque "failed to run migrations" (it refuses to open a DB whose
 * applied migrations the binary doesn't know about). Preferring the repo build
 * in dev makes the UI shell out to the SAME binary the daemon runs, so they can
 * never drift. Production is unchanged: `env !== 'development'` → installed-only.
 */
export function meridianCandidates(
  home: string = process.env.HOME ?? '',
  env: string | undefined = process.env.NODE_ENV,
  cwd: string = process.cwd(),
): string[] {
  const installed = [
    `${home}/.meridian/app/bin/meridian`, // native, no runtime deps — works under launchd
    '/usr/local/bin/meridian', // native, system-wide install
    `${home}/.local/bin/meridian`, // node wrapper — needs `node` on PATH, so last
  ]
  if (env !== 'development') return installed

  // `next dev` runs with cwd = <repo>/ui, so the repo root is one level up.
  const repoRoot = resolve(cwd, '..')
  return [
    `${repoRoot}/target/debug/meridian`, // cargo watch (dev-start.sh) builds here
    `${repoRoot}/target/release/meridian`, // fallback if only a release build exists
    ...installed,
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
