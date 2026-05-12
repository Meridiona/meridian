// meridian — normalises screenpipe activity into structured app sessions

export async function register(): Promise<void> {
  if (process.env.NEXT_RUNTIME === "nodejs") {
    const mod = await import("./lib/observability");
    mod.initOtel();
  }
}
