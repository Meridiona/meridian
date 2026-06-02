import type { NextConfig } from 'next'
import path from 'path'

const nextConfig: NextConfig = {
  // Pin the Turbopack workspace root to this `ui/` directory. The repo root
  // also has a package-lock.json, so Turbopack otherwise infers the monorepo
  // root and tries to watch the whole tree — including the 22 GB Rust
  // `target/` dir — which hangs the dev compile and balloons memory.
  turbopack: {
    root: path.join(__dirname),
  },
  // Emit the self-contained server bundle (`.next/standalone/`) ONLY for the
  // release tarball — gated on MERIDIAN_BUILD_STANDALONE, which .releaserc.json
  // sets for the packaged build. The bundle ships prebuilt and runs with
  // `node server.js` (install-from-bundle.sh) — no `npm ci` / native rebuild of
  // better-sqlite3 on the user's machine.
  //
  // A plain `npm run build` in a source checkout deliberately leaves this unset
  // so it produces a normal build that the launchd dashboard serves with
  // `next start`. Emitting 'standalone' in that path makes `next start` an
  // unsupported combo (noisy startup warning + fragile static-asset serving).
  output: process.env.MERIDIAN_BUILD_STANDALONE ? 'standalone' : undefined,
  serverExternalPackages: [
    'better-sqlite3',
    '@opentelemetry/sdk-node',
    '@opentelemetry/sdk-trace-node',
    '@opentelemetry/sdk-trace-base',
    '@opentelemetry/exporter-trace-otlp-http',
    '@opentelemetry/resources',
    '@opentelemetry/api',
    '@opentelemetry/semantic-conventions',
    'require-in-the-middle',
    'import-in-the-middle',
    'pino',
  ],
  reactStrictMode: true,
  logging: {
    fetches: { fullUrl: false },
  },
}

export default nextConfig
