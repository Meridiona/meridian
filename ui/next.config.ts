import type { NextConfig } from 'next'
import { PHASE_PRODUCTION_BUILD } from 'next/constants'
import path from 'path'

const nextConfig = (phase: string): NextConfig => ({
  // Pin the Turbopack workspace root to this `ui/` directory. The repo root
  // also has a package-lock.json, so Turbopack otherwise infers the monorepo
  // root and tries to watch the whole tree — including the 22 GB Rust
  // `target/` dir — which hangs the dev compile and balloons memory.
  turbopack: {
    root: path.join(__dirname),
  },
  // Static HTML export (Next-fold end-state). The dashboard ships as plain
  // files rendered inside the Tauri webview — no Node server, no `/api` routes
  // (data comes from Rust commands over Tauri `invoke`). The build emits `out/`,
  // which `package.json`'s build step augments with the tray popover under
  // `out/popover/`; `tauri.conf.json` points `frontendDist` at `out/`.
  //
  // Gate on the build phase, NOT process.env.NODE_ENV: a stray NODE_ENV=production
  // in the shell otherwise enables export during `next dev`, which double-joins
  // the dev distDir (`.next/dev/dev`) and panics Turbopack with
  // "Invalid distDirRoot: .next" (vercel/next.js#87881).
  output: phase === PHASE_PRODUCTION_BUILD ? 'export' : undefined,
  // Export has no image optimizer (that needs a server) — serve images as-is.
  images: { unoptimized: true },
  // Emit each route as `<route>/index.html` (e.g. `out/today/index.html`) so the
  // Tauri asset protocol resolves directory-style URLs (`WebviewUrl::App("today")`)
  // and client-side navigation alike.
  trailingSlash: true,
  reactStrictMode: true,
  // Allow the dashboard to be loaded over 127.0.0.1 in dev. Next 16 blocks
  // cross-origin access to internal dev resources (fonts, HMR, /__nextjs_*)
  // unless the requesting host is allow-listed; the dev server trusts
  // `localhost` by default but not the `127.0.0.1` alias the Tauri webview /
  // browser uses. Dev-only — Next ignores this in production builds.
  allowedDevOrigins: ['127.0.0.1', 'localhost'],
  logging: {
    fetches: { fullUrl: false },
  },
})

export default nextConfig
