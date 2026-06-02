import type { NextConfig } from 'next'
import path from 'path'

// ─────────────────────────────────────────────────────────────────────────────
// BUNDLER: dev uses Turbopack (`next dev --turbopack`); the PRODUCTION build uses
// WEBPACK (`next build --webpack`, see package.json). This is deliberate, not a
// leftover. Turbopack's `output: 'standalone'` references serverExternalPackages
// (better-sqlite3, pino, @opentelemetry/*) via hashed symlink aliases under
// `.next/node_modules` (e.g. `better-sqlite3-<hash> -> ../../node_modules/...`).
// Those symlinks are stripped by `npm publish` (our prebuilt bundle ships via the
// @meridiona/meridian-darwin-arm64 npm package), and the hashed alias can fail to
// resolve at runtime even on Vercel — open upstream bugs:
//   https://github.com/vercel/next.js/issues/87737
//   https://github.com/vercel/next.js/issues/93849
// Result was a crash-loop: "Failed to load external module better-sqlite3-<hash>".
// Webpack emits a plain `require("better-sqlite3")` with no symlink indirection,
// which survives npm pack/extract (validated end-to-end). Revert package.json
// `build` to plain `next build` (Turbopack) once those issues close upstream.
// ─────────────────────────────────────────────────────────────────────────────
const nextConfig: NextConfig = {
  // Pin the Turbopack workspace root to this `ui/` directory. The repo root
  // also has a package-lock.json, so Turbopack otherwise infers the monorepo
  // root and tries to watch the whole tree — including the 22 GB Rust
  // `target/` dir — which hangs the dev compile and balloons memory.
  turbopack: {
    root: path.join(__dirname),
  },
  // Emit a self-contained server bundle (`.next/standalone/`) so the dashboard
  // ships prebuilt and runs with `node server.js` — no `npm ci` / native rebuild
  // of better-sqlite3 on the user's machine.
  //
  // NOTE on distribution: the production build uses Turbopack (the Next 16
  // default). Turbopack references serverExternalPackages (better-sqlite3, pino,
  // @opentelemetry/*) via relative SYMLINKS under `.next/node_modules`. Those
  // symlinks are stripped by `npm publish` (vercel/next.js#87737, #93849), which
  // crash-loops the standalone server. So package-release.sh packs the standalone
  // into `ui.tar.gz` (tar preserves symlinks; npm ships it as one opaque file) and
  // install-from-bundle.sh extracts it on the user's machine — keeping Turbopack.
  output: 'standalone',
  // Pin the standalone file-tracing root to this `ui/` dir. The repo root also
  // has a package-lock.json, so Next otherwise infers the monorepo root and
  // nests the output as `.next/standalone/ui/server.js` — which breaks the
  // launchd plist + package-release.sh (both expect `ui/server.js` at the top).
  // Mirrors the `turbopack.root` pin above.
  outputFileTracingRoot: path.join(__dirname),
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
