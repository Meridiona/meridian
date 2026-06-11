//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// SSE endpoint for the global fault bus. The browser opens one persistent
// EventSource connection; this route registers a controller with the shared
// notices-store singleton and keeps the stream open until the tab closes.
//
// One shared setInterval (in notices-store.ts) polls the DB every 30s and
// broadcasts only when the notice set changes — so N open tabs cost one DB
// query per interval, not N.

import { subscribe, unsubscribe } from '@/lib/notices-store'

export const dynamic = 'force-dynamic'

export async function GET() {
  let ctrl: ReadableStreamDefaultController<Uint8Array>

  const stream = new ReadableStream<Uint8Array>({
    start(controller) {
      ctrl = controller
      subscribe(ctrl)
    },
    cancel() {
      unsubscribe(ctrl)
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
