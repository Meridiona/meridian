// meridian — normalises screenpipe activity into structured app sessions

import { NodeSDK } from "@opentelemetry/sdk-node";
import { OTLPTraceExporter } from "@opentelemetry/exporter-trace-otlp-http";
import { Resource } from "@opentelemetry/resources";
import { ATTR_SERVICE_NAME } from "@opentelemetry/semantic-conventions";
import { trace, SpanStatusCode, Span, Attributes } from "@opentelemetry/api";
import pino from "pino";

let _sdk: NodeSDK | null = null;
const _serviceName = "meridian-ui";

export function initOtel(): void {
  if (_sdk) return;

  const endpoint =
    process.env.MERIDIAN_OTLP_TRACES_ENDPOINT ??
    "http://localhost:5080/api/default/v1/traces";

  const headers: Record<string, string> = {};
  if (process.env.MERIDIAN_OO_AUTH) {
    headers["Authorization"] = `Basic ${process.env.MERIDIAN_OO_AUTH}`;
  }

  const exporter = new OTLPTraceExporter({ url: endpoint, headers });

  _sdk = new NodeSDK({
    resource: new Resource({ [ATTR_SERVICE_NAME]: _serviceName }),
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
