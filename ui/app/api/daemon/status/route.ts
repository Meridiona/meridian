// meridian — normalises screenpipe activity into structured app sessions
//
// GET /api/daemon/status — lightweight liveness probe for the daemon socket.
// Used by the Settings UI to poll during a reload and show "Active" when
// the daemon comes back up.

import net from 'net'
import os from 'os'
import path from 'path'

export async function GET() {
  const result = await new Promise<{ running: boolean; pid?: number }>((resolve) => {
    const sockPath = path.join(os.homedir(), '.meridian', 'daemon.sock')
    const socket = net.connect(sockPath)
    let buf = ''
    const timer = setTimeout(() => {
      socket.destroy()
      resolve({ running: false })
    }, 800)
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
  return Response.json(result)
}
