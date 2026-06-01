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
  // Emit a self-contained server bundle (`.next/standalone/`) so the dashboard
  // ships prebuilt in the release tarball and runs with `node server.js` — no
  // `npm ci` / native rebuild of better-sqlite3 on the user's machine.
  output: 'standalone',
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
