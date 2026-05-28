import type { NextConfig } from 'next'

const nextConfig: NextConfig = {
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
