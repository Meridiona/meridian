//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Copy the tray popover sources (tray/src) into the Next.js tree so the
// popover window resolves. Two modes, two destinations:
//
//   dev   → ui/public/popover   (next dev serves public/ at the root)
//   build → ui/out/popover      (the static export the packaged app loads)
//
// This replaces two Unix-only shell copies that failed on Windows:
//   * tauri.conf.json's beforeDevCommand: `rm -rf … && mkdir -p … && cp -r …`,
//     run through `cmd /C` on Windows where none of those commands exist;
//   * ui/package.json's build: `next build && cp -r ../tray/src out/popover`,
//     run inside `tauri build`'s beforeBuildCommand, so a Windows package
//     build died right after `next build`.
//
// Node already runs both the dev server and the build, so this adds no
// dependency and behaves identically on every platform. Paths are resolved
// from THIS file's location, so the caller's working directory does not
// matter (dev runs it from tray/, the build from ui/).

import { cpSync, mkdirSync, rmSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const mode = process.argv[2] ?? "dev";
const DESTS = { dev: ["public", "popover"], build: ["out", "popover"] };
const rel = DESTS[mode];
if (!rel) {
  console.error(`sync-popover: unknown mode "${mode}" (expected "dev" or "build")`);
  process.exit(1);
}

const trayDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const src = resolve(trayDir, "src");
const dest = resolve(trayDir, "..", "ui", ...rel);

// `force` so a first run (nothing to delete) is not an error, and `recursive`
// so a populated directory from a previous run goes cleanly.
rmSync(dest, { recursive: true, force: true });
mkdirSync(dest, { recursive: true });
cpSync(src, dest, { recursive: true });

console.log(`✓ popover synced (${mode}): ${src} → ${dest}`);
