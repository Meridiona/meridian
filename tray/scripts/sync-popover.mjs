//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Copy the tray popover sources into the Next.js public dir so `tauri dev`
// can serve them.
//
// This replaces `rm -rf … && mkdir -p … && cp -r …` in tauri.conf.json's
// beforeDevCommand. Tauri runs that string through the platform shell — `sh -c`
// on Unix but `cmd /C` on Windows — where none of those three commands exist,
// so the dev server never starts. Node is already required to run the dev
// server itself, so doing it here costs no new dependency and behaves
// identically on every platform.
//
// Why the copy exists at all: the popover's source of truth is tray/src/, but
// the main window loads it from the Next output (`popover/index.html`). In a
// packaged build the Tauri build step copies it into ui/out/popover/; in dev,
// next serves ui/public/, so it has to land there instead. See CLAUDE.md's
// "Asset layout" note.

import { cpSync, mkdirSync, rmSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const trayDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const src = resolve(trayDir, "src");
const dest = resolve(trayDir, "..", "ui", "public", "popover");

// `force` so a first run (nothing to delete) is not an error, and `recursive`
// so a populated directory from a previous run goes cleanly.
rmSync(dest, { recursive: true, force: true });
mkdirSync(dest, { recursive: true });
cpSync(src, dest, { recursive: true });

console.log(`✓ popover synced: ${src} → ${dest}`);
