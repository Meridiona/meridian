// meridian — normalises screenpipe activity into structured app sessions

export async function register(): Promise<void> {
  // Only the Node.js server runtime in production can host the OTel SDK.
  if (process.env.NEXT_RUNTIME !== "nodejs" || process.env.NODE_ENV !== "production") {
    return;
  }
  // Telemetry is OFF by default. Gate the *import* — not just initOtel() — on the
  // opt-in flag: ./lib/observability statically imports @opentelemetry/sdk-node,
  // which is a serverExternalPackage that may be absent from the standalone
  // bundle. Importing it unconditionally throws "Cannot find module
  // @opentelemetry/sdk-node", crashing the instrumentation hook and 500-ing every
  // route — even when tracing is disabled. So when it's off we never load OTel at
  // all; when it's on we fail soft so the dashboard still serves if OTel is broken.
  const enabled = process.env.MERIDIAN_OTEL_ENABLED;
  if (enabled !== "1" && enabled !== "true") {
    return;
  }
  try {
    const mod = await import("./lib/observability");
    mod.initOtel();
  } catch (err) {
    console.error(
      "meridian-ui: OpenTelemetry instrumentation unavailable — serving without tracing:",
      err,
    );
  }
}
