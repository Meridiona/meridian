#!/usr/bin/env python3
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
"""Merge a windows-x86_64 entry into an already-published latest.json.

The macOS release job publishes latest.json with both darwin-* keys. The
Windows job runs afterwards, on a different runner, and has to ADD its own
platform key to that same manifest without disturbing what is already there.

This is the single most dangerous step in the Windows release path: it is the
only place where a Windows-side bug can break **macOS** auto-update, by
re-uploading a manifest that has lost or corrupted the darwin entries. So it
refuses to write anything it cannot first prove is safe:

  * the fetched manifest must parse, and carry both darwin-aarch64 and
    darwin-x86_64 with a signature and url each;
  * the version must match the release being built;
  * the result must still carry both darwin entries, byte-identical to the
    ones that came in.

Any of those failing is a hard error, never a warning — publishing a
half-correct manifest is worse than publishing no Windows entry at all,
because every installed macOS app polls this file.

Usage:
    merge-windows-updater.py <latest.json> <version> <installer-url> <sig-file>
"""

import json
import sys
from pathlib import Path

REQUIRED_DARWIN = ("darwin-aarch64", "darwin-x86_64")


def die(msg: str) -> "None":
    sys.exit(f"✗ merge-windows-updater: {msg}")


def main() -> None:
    if len(sys.argv) != 5:
        die("usage: merge-windows-updater.py <latest.json> <version> <url> <sig-file>")

    manifest_path = Path(sys.argv[1])
    version = sys.argv[2].lstrip("v")
    url = sys.argv[3]
    sig_path = Path(sys.argv[4])

    try:
        manifest = json.loads(manifest_path.read_text())
    except (OSError, json.JSONDecodeError) as e:
        die(f"cannot read {manifest_path}: {e}")

    platforms = manifest.get("platforms")
    if not isinstance(platforms, dict):
        die("manifest has no 'platforms' object — refusing to write")

    # Capture the incoming darwin entries so the post-write check can prove
    # they survived untouched rather than merely being present.
    before = {}
    for key in REQUIRED_DARWIN:
        entry = platforms.get(key)
        if not isinstance(entry, dict) or not entry.get("signature") or not entry.get("url"):
            die(
                f"manifest is missing a usable '{key}' entry. The macOS job "
                f"publishes it; if it is absent here, that job did not run or "
                f"did not finish. Refusing to write."
            )
        before[key] = dict(entry)

    if manifest.get("version") != version:
        die(
            f"manifest version {manifest.get('version')!r} != release version "
            f"{version!r} — this is a manifest from a DIFFERENT release. "
            f"Writing would point macOS users at the wrong build."
        )

    try:
        signature = sig_path.read_text().strip()
    except OSError as e:
        die(f"cannot read signature {sig_path}: {e}")
    if not signature:
        die(f"signature file {sig_path} is empty")

    platforms["windows-x86_64"] = {"signature": signature, "url": url}

    # Prove the darwin entries are exactly as they arrived.
    for key, original in before.items():
        if platforms.get(key) != original:
            die(f"'{key}' changed during merge — refusing to write")

    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")
    covered = ", ".join(sorted(platforms))
    print(f"✓ merge-windows-updater: {manifest_path} now covers {covered}")


if __name__ == "__main__":
    main()
