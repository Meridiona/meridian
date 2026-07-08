//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// DOM harness test for the tray popover's "Capturing" health panel
// (tray/src/app.js). The popover is plain HTML+JS loaded into a Tauri
// WKWebView; there is no WebDriver path for a menu-bar NSPanel on macOS, so
// this is the closest achievable automated guard: load the REAL app.js into a
// happy-dom document with a mocked __TAURI__ bridge and drive its exposed
// functions.
//
// What it locks in (the behaviour PR "click Capturing to show app status" added):
//   1. The health panel is hidden until toggled.
//   2. Clicking / toggling head-sub reveals it and flips aria-expanded.
//   3. refreshHealth() paints the daemon / MLX / database rows + status dots
//      from the get_daemon_status / get_mlx_status / get_health responses,
//      including the snake_case MlxStatus contract ("running" → green "Running").
//   4. A null (unreachable) command result degrades to a "bad" dot, never throws.
//
// app.js self-exposes { render, toggleHealth, refreshHealth, paint* } on
// globalThis.__popover when globalThis.__MERIDIAN_POPOVER_TEST__ is set, and
// skips its auto-boot (ticker + get_status) so importing it here is side-effect
// free.

import { describe, it, expect, beforeAll, afterAll, beforeEach } from 'bun:test'
import { readFileSync } from 'fs'
import { GlobalRegistrator } from '@happy-dom/global-registrator'

const trayDir = import.meta.dir + '/../../tray/src'

// Extract the popover markup from index.html (minus its <script> tags, which we
// load manually) so the elements app.js queries by id actually exist.
function popoverBody(): string {
  const html = readFileSync(trayDir + '/index.html', 'utf8')
  const m = html.match(/<body[^>]*>([\s\S]*)<\/body>/i)
  return (m ? m[1] : '').replace(/<script[\s\S]*?<\/script>/gi, '')
}

const pauseUtilsSrc = readFileSync(trayDir + '/pause-utils.js', 'utf8')
const appSrc = readFileSync(trayDir + '/app.js', 'utf8')

type InvokeImpl = (cmd: string, args?: unknown) => Promise<unknown>
let invokeImpl: InvokeImpl = async () => null

// The popover object app.js exposes under the test flag.
interface Popover {
  render: (status: Record<string, unknown>) => void
  toggleHealth: () => void
  refreshHealth: () => Promise<void>
  paintDaemon: (r: unknown) => void
  paintMlx: (r: unknown) => void
  paintDb: (r: unknown) => void
}

function popover(): Popover {
  return (globalThis as unknown as { __popover: Popover }).__popover
}

function loadPopover() {
  document.body.innerHTML = popoverBody()
  ;(globalThis as Record<string, unknown>).__MERIDIAN_POPOVER_TEST__ = true
  ;(globalThis as Record<string, unknown>).__TAURI__ = {
    core: { invoke: (cmd: string, args?: unknown) => invokeImpl(cmd, args) },
    event: { listen: async () => () => {} },
    window: {
      // resizeToContent early-returns on a 0-height layout, so this is only a
      // safety net if that guard ever changes.
      getCurrentWindow: () => ({ setSize: async () => {} }),
      LogicalSize: class {
        constructor(public w: number, public h: number) {}
      },
    },
  }
  // One combined eval so app.js's references to pause-utils' globals resolve in
  // the same scope; app.js self-exposes globalThis.__popover at the end.
  ;(0, eval)(pauseUtilsSrc + '\n' + appSrc)
}

const dot = (id: string) => document.getElementById(id)?.getAttribute('data-state')
const text = (id: string) => document.getElementById(id)?.textContent

beforeAll(() => GlobalRegistrator.register())
afterAll(() => GlobalRegistrator.unregister())

beforeEach(() => {
  invokeImpl = async () => null
  loadPopover()
})

describe('popover health panel', () => {
  it('is hidden until toggled', () => {
    expect(document.getElementById('health-panel')?.hasAttribute('hidden')).toBe(true)
    expect(document.getElementById('head-sub')?.getAttribute('aria-expanded')).toBe('false')
  })

  it('toggleHealth reveals the panel and flips aria-expanded, then hides again', () => {
    popover().toggleHealth()
    expect(document.getElementById('health-panel')?.hasAttribute('hidden')).toBe(false)
    expect(document.getElementById('head-sub')?.getAttribute('aria-expanded')).toBe('true')

    popover().toggleHealth()
    expect(document.getElementById('health-panel')?.hasAttribute('hidden')).toBe(true)
    expect(document.getElementById('head-sub')?.getAttribute('aria-expanded')).toBe('false')
  })

  it('paints all-healthy state: daemon running, MLX running (snake_case), db ready', async () => {
    invokeImpl = async (cmd) => {
      switch (cmd) {
        case 'get_daemon_status':
          return { running: true, pid: 4242 }
        case 'get_mlx_status':
          // Mirrors MlxStatusResponse: MlxStatus serializes snake_case → "running".
          return { status: 'running', port: 7823, runtime_installed: true }
        case 'get_health':
          return { database_ready: true }
        default:
          return null
      }
    }
    await popover().refreshHealth()

    expect(dot('hp-daemon-dot')).toBe('ok')
    expect(text('hp-daemon-val')).toBe('Running (pid 4242)')
    expect(dot('hp-mlx-dot')).toBe('ok')
    expect(text('hp-mlx-val')).toBe('Running (port 7823)')
    expect(dot('hp-db-dot')).toBe('ok')
    expect(text('hp-db-val')).toBe('Ready')
  })

  it('paints a downed daemon as a bad dot with "Not running"', async () => {
    invokeImpl = async (cmd) =>
      cmd === 'get_daemon_status' ? { running: false } : null
    await popover().refreshHealth()
    expect(dot('hp-daemon-dot')).toBe('bad')
    expect(text('hp-daemon-val')).toBe('Not running')
  })

  it('paints "Runtime not installed" (warn) when MLX is offline with no runtime', async () => {
    invokeImpl = async (cmd) =>
      cmd === 'get_mlx_status'
        ? { status: 'offline', port: 7823, runtime_installed: false }
        : null
    await popover().refreshHealth()
    expect(dot('hp-mlx-dot')).toBe('warn')
    expect(text('hp-mlx-val')).toBe('Runtime not installed')
  })

  it('degrades to "Unreachable" (bad dot) when a command returns null, never throws', async () => {
    invokeImpl = async () => null
    await popover().refreshHealth()
    expect(dot('hp-daemon-dot')).toBe('bad')
    expect(text('hp-daemon-val')).toBe('Unreachable')
    expect(dot('hp-mlx-dot')).toBe('bad')
    expect(text('hp-mlx-val')).toBe('Unreachable')
    expect(dot('hp-db-dot')).toBe('bad')
    expect(text('hp-db-val')).toBe('Unreachable')
  })

  it('surfaces a database error message on the db row', async () => {
    invokeImpl = async (cmd) =>
      cmd === 'get_health' ? { database_ready: false, error: 'disk I/O error' } : null
    await popover().refreshHealth()
    expect(dot('hp-db-dot')).toBe('bad')
    expect(text('hp-db-val')).toBe('disk I/O error')
  })

  it('surfaces the MLX Error(String) message, not a bare "Error"', async () => {
    // MlxStatus::Error(String) serializes externally-tagged snake_case →
    // { status: { error: "<msg>" } }. The row should show the message.
    invokeImpl = async (cmd) =>
      cmd === 'get_mlx_status'
        ? { status: { error: 'model failed to load' }, port: 7823, runtime_installed: true }
        : null
    await popover().refreshHealth()
    expect(dot('hp-mlx-dot')).toBe('bad')
    expect(text('hp-mlx-val')).toBe('Error: model failed to load')
  })
})
