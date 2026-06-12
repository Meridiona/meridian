//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// POST /api/daemon/reload — send SIGHUP to the running daemon.
// The daemon exits cleanly on SIGHUP; launchd restarts it automatically,
// picking up any settings.json changes (OTLP config, credentials, etc.).
// Log-level changes are hot-reloaded in-process and do not need this.

import net from 'net'
import os from 'os'
import path from 'path'

function readDaemonSocket(): Promise<{ running: boolean; pid?: number }> {
  return new Promise((resolve) => {
    const sockPath = path.join(os.homedir(), '.meridian', 'daemon.sock')
    const socket = net.connect(sockPath)
    let buf = ''
    const timer = setTimeout(() => {
      socket.destroy()
      resolve({ running: false })
    }, 1000)
    socket.on('data', (chunk) => { buf += chunk })
    socket.on('end', () => {
      clearTimeout(timer)
      try {
        const json = JSON.parse(buf)
        resolve({ running: true, pid: json.pid as number })
      } catch {
        resolve({ running: false })
      }
    })
    socket.on('error', () => {
      clearTimeout(timer)
      resolve({ running: false })
    })
  })
}

export async function POST() {
  const { running, pid } = await readDaemonSocket()
  if (!running || !pid) {
    return Response.json({ error: 'daemon not running' }, { status: 503 })
  }
  try {
    process.kill(pid, 'SIGHUP')
    return Response.json({ ok: true, pid })
  } catch (e) {
    return Response.json({ error: String(e) }, { status: 500 })
  }
}
