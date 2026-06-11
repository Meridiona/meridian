//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

import { NodeSDK } from "@opentelemetry/sdk-node";
import { OTLPTraceExporter } from "@opentelemetry/exporter-trace-otlp-http";
import { trace, SpanStatusCode, Span, Attributes } from "@opentelemetry/api";
import pino from "pino";

let _sdk: NodeSDK | null = null;
const _serviceName = "meridian-ui";

export function initOtel(): void {
  if (_sdk) return;

  // UI trace export is OFF by default — opt in with MERIDIAN_OTEL_ENABLED=1 in
  // ui/.env.local. This is per-service: the Rust daemon and Python services have
  // their own exporters and are unaffected. Logging (pino, LOG_LEVEL) is too.
  const enabled = process.env.MERIDIAN_OTEL_ENABLED;
  if (enabled !== "1" && enabled !== "true") return;

  const endpoint =
    process.env.MERIDIAN_OTLP_TRACES_ENDPOINT ??
    "http://localhost:5080/api/default/v1/traces";

  const headers: Record<string, string> = {};
  if (process.env.MERIDIAN_OO_AUTH) {
    headers["Authorization"] = `Basic ${process.env.MERIDIAN_OO_AUTH}`;
  }

  const exporter = new OTLPTraceExporter({ url: endpoint, headers });

  // serviceName lets NodeSDK build the resource internally — avoids a direct
  // @opentelemetry/resources dep whose version can drift from sdk-node's own
  // (the dual-package type clash that broke the v1.24.6 release build).
  _sdk = new NodeSDK({
    serviceName: _serviceName,
    traceExporter: exporter,
  });

  try {
    _sdk.start();
  } catch (err) {
    // Avoid throwing during Next.js boot; surface to stderr instead.
    console.error("OTel init failed:", err);
  }

  const shutdown = async () => {
    try {
      await _sdk?.shutdown();
    } catch {
      // ignore
    }
  };
  process.once("SIGTERM", shutdown);
  process.once("SIGINT", shutdown);
}

// UI server writes logs to stdout (no JSON-RPC channel to protect).
export const logger = pino({
  level: process.env.LOG_LEVEL ?? "info",
  base: { service: _serviceName },
  mixin() {
    const span = trace.getActiveSpan();
    if (!span) return {};
    const ctx = span.spanContext();
    return { trace_id: ctx.traceId, span_id: ctx.spanId };
  },
});

export async function withSpan<T>(
  name: string,
  attrs: Attributes,
  fn: (span: Span) => Promise<T>,
): Promise<T> {
  const tracer = trace.getTracer(_serviceName);
  return tracer.startActiveSpan(name, { attributes: attrs }, async (span) => {
    try {
      const result = await fn(span);
      span.setStatus({ code: SpanStatusCode.OK });
      return result;
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      span.recordException(err instanceof Error ? err : new Error(msg));
      span.setStatus({ code: SpanStatusCode.ERROR, message: msg });
      throw err;
    } finally {
      span.end();
    }
  });
}
