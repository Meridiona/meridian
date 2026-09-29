//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
import { cloudflareTest } from "@cloudflare/vitest-plugin";
import { defineConfig } from "vitest/config";

// Runs tests inside a real Miniflare-simulated Workers runtime (Web Crypto,
// KVNamespace, etc. all behave like the deployed Worker) rather than plain
// Node — chosen over hand-mocked globals because crypto-utils.ts's
// `crypto.subtle.timingSafeEqual` is a Workers-runtime API, not a standard
// Node one. No Cloudflare account or network access needed: Miniflare runs
// entirely locally, reading only `wrangler.jsonc` for binding shapes.
//
// `@cloudflare/vitest-plugin` (not the older `@cloudflare/vitest-pool-workers`,
// which has no `defineWorkersConfig` export as of Vitest 4 / pool-workers
// 0.22.x) is Cloudflare's Vite-plugin-based successor for Vitest v4+.
export default defineConfig({
  plugins: [
    cloudflareTest({
      wrangler: { configPath: "./wrangler.jsonc" },
    }),
  ],
});
