//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

import { NodeSDK } from "@opentelemetry/sdk-node";
import { OTLPTraceExporter } from "@opentelemetry/exporter-trace-otlp-http";
import { Resource } from "@opentelemetry/resources";
import { ATTR_SERVICE_NAME } from "@opentelemetry/semantic-conventions";
import { trace, SpanStatusCode, Span, Attributes } from "@opentelemetry/api";
import pino from "pino";

let _sdk: NodeSDK | null = null;
let _serviceName = "meridian-mcp";

export function initOtel(serviceName: string): void {
  if (_sdk) return;
  _serviceName = serviceName;

  const endpoint =
    process.env.MERIDIAN_OTLP_TRACES_ENDPOINT ??
    "http://localhost:5080/api/default/v1/traces";

  const headers: Record<string, string> = {};
  if (process.env.MERIDIAN_OO_AUTH) {
    headers["Authorization"] = `Basic ${process.env.MERIDIAN_OO_AUTH}`;
  }

  const exporter = new OTLPTraceExporter({ url: endpoint, headers });

  _sdk = new NodeSDK({
    resource: new Resource({ [ATTR_SERVICE_NAME]: serviceName }),
    traceExporter: exporter,
  });

  try {
    _sdk.start();
  } catch (err) {
    // Logging via console.error here is safe (stderr) and avoids a chicken-and-egg
    // problem with the pino logger which depends on the OTel API for trace ids.
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

// pino writes to stderr (fd 2) so it never pollutes the JSON-RPC stdout channel.
export const logger = pino(
  {
    level: process.env.LOG_LEVEL ?? "info",
    mixin() {
      const span = trace.getActiveSpan();
      if (!span) return {};
      const ctx = span.spanContext();
      return { trace_id: ctx.traceId, span_id: ctx.spanId };
    },
  },
  pino.destination(2),
);

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
