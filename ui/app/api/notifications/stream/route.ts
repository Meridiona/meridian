//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// SSE endpoint for the in-app notification banner channel. Mirrors
// /api/notices/stream: the browser opens one EventSource; this registers a
// controller with the shared banner store and streams the active banner set.

import { subscribe, unsubscribe } from '@/lib/notifications-banner-store'

export const dynamic = 'force-dynamic'

export async function GET() {
  let ctrl: ReadableStreamDefaultController<Uint8Array>
  const stream = new ReadableStream<Uint8Array>({
    start(controller) { ctrl = controller; subscribe(ctrl) },
    cancel() { unsubscribe(ctrl) },
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
