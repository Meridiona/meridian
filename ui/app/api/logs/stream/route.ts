//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// SSE endpoint that tails the daemon's JSONL log file. Uses kqueue/inotify
// via fs.watch() — reliable for append-only files. N open tabs share one
// watcher and one file position; no per-connection polling overhead.

import { subscribeToTail, unsubscribeFromTail } from '@/lib/log-tail'

export const dynamic = 'force-dynamic'

export async function GET() {
  let ctrl: ReadableStreamDefaultController<Uint8Array>

  const stream = new ReadableStream<Uint8Array>({
    start(controller) {
      ctrl = controller
      subscribeToTail(ctrl)
    },
    cancel() {
      unsubscribeFromTail(ctrl)
    },
  })

  return new Response(stream, {
    headers: {
      'Content-Type': 'text/event-stream',
      'Cache-Control': 'no-cache, no-transform',
      Connection: 'keep-alive',
      'X-Accel-Buffering': 'no',
    },
  })
}
