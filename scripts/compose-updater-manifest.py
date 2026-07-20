#!/usr/bin/env python3
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
#
# Compose ONE tauri-plugin-updater manifest (latest.json) from the per-platform
# fragments each build runner uploaded to the draft release.
#
#   scripts/compose-updater-manifest.py \
#       --version 1.73.0 --tag v1.73.0 --fragments frag/ --output latest.json
#
# ── WHY THIS EXISTS ──────────────────────────────────────────────────────────
#
# Because the obvious alternative is a data-loss race.
#
# tauri-action's built-in updater-manifest upload (src/upload-version-json.ts)
# runs ONCE PER MATRIX JOB and does a read-modify-write against a single shared
# release asset with NO locking:
#
#     list release assets -> find latest.json -> download it -> parse it ->
#     seed platforms from it -> add my platform -> DELETE the old asset ->
#     upload the new one
#
# Two runners finishing inside the same window each read the pre-merge manifest,
# and the one that writes second silently drops the other's platform key. The
# action's own source concedes it in a comment. In our shape the visible symptom
# would be "Intel users never received the update" — a release that looks
# completely green.
#
# Composing once, after every runner has finished, removes the race by
# construction. It also gives exactly one place that knows the FULL expected key
# set, which is what makes the completeness assertion in the publish job
# possible at all: you cannot assert a manifest is complete from inside a job
# that only knows about its own architecture.
#
# ── FRAGMENT CONTRACT ────────────────────────────────────────────────────────
#
# Each build runner writes a fragment named for its target triple, so two
# runners can never collide and no arch-label mapping table has to be kept in
# sync (tauri spells macOS Intel "x64" in bundle filenames but "x86_64" in
# updater platform keys — naming fragments by triple sidesteps that entirely):
#
#     updater-aarch64-apple-darwin.json
#     updater-x86_64-apple-darwin.json
#     updater-windows.json
#
# Each contains the standard manifest shape carrying ONLY its own platform
# key(s), e.g.
#
#     {"version": "...", "notes": "...", "pub_date": "...",
#      "platforms": {"darwin-aarch64": {"signature": "...", "url": "..."}}}
#
# ── WHAT THIS SCRIPT REFUSES TO DO ───────────────────────────────────────────
#
# It will not emit a manifest containing a platform entry with an empty or
# missing `signature` or `url`. A half-populated entry is worse than an absent
# one: tauri-plugin-updater will hand it to a user's app, which then fails to
# verify or fails to download, and the app has no way to fall back. Better to
# fail the release while it is still a draft.
import argparse
import json
import pathlib
import re
import sys
from datetime import datetime, timezone

# Every key we expect a complete release to carry. Kept here rather than in the
# workflow so the manifest's shape has one owner.
#
# The `-nsis` alias is not redundant: tauri-plugin-updater >= 2.10 also looks up
# {os}-{arch}-{installer} keys, and tauri-action's own updaterJsonPreferNsis
# defaults to FALSE "for legacy reasons" — meaning MSI wins over NSIS when both
# are present. We ship NSIS, so we publish both spellings pointing at the same
# installer and the preference never gets a chance to pick wrong.
REQUIRED = ("darwin-aarch64", "darwin-x86_64", "windows-x86_64")

MINIMUM_RE = re.compile(r"^Minimum-Version:\s*\d+\.\d+\.\d+\s*$", re.MULTILINE)


def load_fragments(directory: pathlib.Path) -> list[tuple[pathlib.Path, dict]]:
    """Every updater-*.json in `directory`, parsed, sorted for stable output.

    A fragment that is present but unparseable is fatal rather than skipped —
    silently ignoring it is how a platform goes missing from a green release.
    """
    found = []
    for path in sorted(directory.glob("updater-*.json")):
        try:
            found.append((path, json.loads(path.read_text())))
        except json.JSONDecodeError as exc:
            sys.exit(f"::error::{path.name} is not valid JSON: {exc}")
    return found


def pick_notes(fragments: list[tuple[pathlib.Path, dict]], version: str) -> str:
    """Release notes for the manifest, preserving any Minimum-Version floor.

    The floor is transported INSIDE the notes body because tauri-plugin-updater
    drops manifest fields it does not recognise — see scripts/package-updater.sh
    and tray/src-tauri/src/update.rs (enforce_minimum_version). Losing it here
    would silently turn a mandatory release back into a consent-based one, which
    is exactly the kind of failure nobody notices until it matters.

    Fragments are generated per-runner from the same tray/minimum-version file,
    so any fragment carrying a floor is authoritative; prefer one that has it.
    """
    for _, frag in fragments:
        notes = frag.get("notes", "")
        if MINIMUM_RE.search(notes):
            return notes
    for _, frag in fragments:
        if frag.get("notes"):
            return frag["notes"]
    return f"Meridian v{version}"


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--version", required=True, help="release version, no leading v")
    ap.add_argument("--tag", required=True, help="the v* tag being published")
    ap.add_argument("--fragments", required=True, type=pathlib.Path)
    ap.add_argument("--output", required=True, type=pathlib.Path)
    ap.add_argument(
        "--allow-partial",
        action="store_true",
        help="emit a manifest even if a REQUIRED platform key is absent "
        "(for a deliberate single-platform hotfix; never in normal use)",
    )
    args = ap.parse_args()

    version = args.version.lstrip("v")

    fragments = load_fragments(args.fragments)
    if not fragments:
        sys.exit(f"::error::no updater-*.json fragments found in {args.fragments}")

    platforms: dict[str, dict] = {}
    for path, frag in fragments:
        # A fragment stamped with a different version than the release means a
        # stale asset survived on the draft from an earlier attempt. Publishing
        # it would point users at a payload that is not this release.
        frag_version = str(frag.get("version", "")).lstrip("v")
        if frag_version and frag_version != version:
            sys.exit(
                f"::error::{path.name} is stamped {frag_version} but this release "
                f"is {version} — a stale fragment survived on the draft release. "
                f"Delete it and re-run the platform build."
            )

        # A fragment with no `platforms` object at all is MALFORMED, not empty.
        # Treating it as empty would make it contribute nothing and surface
        # later as a confusing "missing platform X" from the completeness check,
        # pointing at the wrong culprit. Say what is actually wrong, here.
        if "platforms" not in frag:
            sys.exit(
                f"::error::{path.name} has no 'platforms' object. A fragment must be a "
                f"full manifest carrying only its own platform keys, e.g. "
                f'{{"platforms": {{"darwin-aarch64": {{"signature": "...", "url": "..."}}}}}}'
            )

        for key, entry in (frag.get("platforms") or {}).items():
            signature = (entry or {}).get("signature", "").strip()
            url = (entry or {}).get("url", "").strip()
            if not signature or not url:
                sys.exit(
                    f"::error::{path.name} carries platform '{key}' with an empty "
                    f"{'signature' if not signature else 'url'}. Refusing to publish "
                    f"a manifest an installed app cannot act on."
                )
            if key in platforms and platforms[key] != {"signature": signature, "url": url}:
                sys.exit(
                    f"::error::platform '{key}' is claimed by more than one fragment "
                    f"with different contents (last seen in {path.name}). Two runners "
                    f"believe they own the same platform."
                )
            platforms[key] = {"signature": signature, "url": url}

    missing = [k for k in REQUIRED if k not in platforms]
    if missing:
        message = (
            f"::error::latest.json would be missing required platform(s): "
            f"{', '.join(missing)}. Fragments present: "
            f"{', '.join(p.name for p, _ in fragments)}"
        )
        if not args.allow_partial:
            sys.exit(message)
        print(message.replace("::error::", "::warning::"))

    manifest = {
        "version": version,
        "notes": pick_notes(fragments, version),
        # RFC 3339 / ISO 8601 in UTC, which is what tauri-plugin-updater parses.
        # Generated here rather than taken from a fragment because each runner
        # stamps its own finish time and the manifest should carry one date.
        "pub_date": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        # Sorted so a diff between two releases shows real changes only.
        "platforms": dict(sorted(platforms.items())),
    }

    args.output.write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"✓ {args.output} — {len(platforms)} platform keys for {args.tag}")
    for key in sorted(platforms):
        print(f"    {key}")


if __name__ == "__main__":
    main()
