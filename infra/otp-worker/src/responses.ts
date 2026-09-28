//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
/**
 * Small, consistent JSON response builders for the status codes
 * `/otp/capture` actually uses (400/401/404/429/503) — kept out of
 * `index.ts` so the handler reads as "gate, gate, gate, do the thing" rather
 * than inline `Response.json(...)` literals.
 *
 * # Related
 * - `index.ts` — the only caller
 * - README.md — the status-code-to-meaning mapping, documented there since
 *   the plan states the set of codes but not the full route-by-route mapping
 */

function jsonResponse(body: unknown, status: number): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

export const notFound = (): Response => jsonResponse({ error: "not_found" }, 404);
export const unauthorized = (): Response => jsonResponse({ error: "unauthorized" }, 401);
export const badRequest = (error: string): Response => jsonResponse({ error }, 400);
export const rateLimited = (scope: string): Response => jsonResponse({ error: "rate_limited", scope }, 429);
export const serviceUnavailable = (error: string): Response => jsonResponse({ error }, 503);
export const ok = (body: Record<string, unknown> = {}): Response => jsonResponse({ ok: true, ...body }, 200);
