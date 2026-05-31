import type { NextConfig } from 'next'

const nextConfig: NextConfig = {
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
