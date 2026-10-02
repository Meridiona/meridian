#!/usr/bin/env python3
"""Validate and stage an RC asset set for byte-preserving stable promotion.

This script never talks to GitHub. The promotion workflow supplies release API
metadata and downloaded assets, which keeps the integrity rules independently
testable and makes every network mutation visible in the workflow.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import re
import sys
from typing import Any, NoReturn


RC_RE = re.compile(r"^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)-rc\.([1-9]\d*)$")
VERSION_RE = re.compile(r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$")
SHA_RE = re.compile(r"^[0-9a-f]{40}$")
MINIMUM_RE = re.compile(r"(?m)^Minimum-Version: ([0-9]+\.[0-9]+\.[0-9]+)\s*$")
MINIMUM_LINE_RE = re.compile(r"(?m)^Minimum-Version:.*$")

MANIFEST_PLATFORMS = {
    "updater-aarch64-apple-darwin.json": {"darwin-aarch64"},
    "updater-windows.json": {"windows-x86_64", "windows-x86_64-nsis"},
    "latest.json": {"darwin-aarch64", "windows-x86_64", "windows-x86_64-nsis"},
}

REQUIRED_ASSETS = {
    "Meridian-aarch64.app.tar.gz",
    "Meridian-aarch64.app.tar.gz.sig",
    "Meridian-aarch64.dmg",
    "Meridian-x86_64-setup.exe",
    "Meridian-x86_64-setup.exe.sig",
    "latest.json",
    "smoke-macos.ok",
    "smoke-windows.ok",
    "updater-aarch64-apple-darwin.json",
    "updater-windows.json",
}


def fail(message: str) -> NoReturn:
    raise SystemExit(f"::error::{message}")


def read_json(path: pathlib.Path) -> Any:
    try:
        return json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as exc:
        fail(f"cannot read valid JSON from {path}: {exc}")


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def release_assets(release: dict[str, Any]) -> dict[str, dict[str, Any]]:
    result: dict[str, dict[str, Any]] = {}
    for asset in release.get("assets") or []:
        name = asset.get("name")
        if not isinstance(name, str) or not name or name in {".", ".."}:
            fail("release contains an asset with an invalid name")
        if "/" in name or "\\" in name or "\n" in name or "\r" in name:
            fail(f"unsafe release asset name: {name!r}")
        if name in result:
            fail(f"release contains duplicate asset name {name}")
        result[name] = asset
    return result


def validate_source_files(
    assets_dir: pathlib.Path, release: dict[str, Any]
) -> dict[str, dict[str, Any]]:
    metadata = release_assets(release)
    missing = sorted(REQUIRED_ASSETS - metadata.keys())
    if missing:
        fail(f"RC is missing required assets: {', '.join(missing)}")

    # JSON assets participate in updater behavior. Unknown ones cannot safely be
    # copied because their embedded RC URLs would not have a defined rewrite.
    unexpected_json = sorted(name for name in metadata if name.endswith(".json") and name not in MANIFEST_PLATFORMS)
    if unexpected_json:
        fail(f"RC contains unknown JSON assets: {', '.join(unexpected_json)}")

    disk_names = {path.name for path in assets_dir.iterdir() if path.is_file()}
    if disk_names != set(metadata):
        missing_disk = sorted(set(metadata) - disk_names)
        extra_disk = sorted(disk_names - set(metadata))
        fail(f"downloaded asset set differs from the RC (missing={missing_disk}, extra={extra_disk})")

    for name, asset in metadata.items():
        path = assets_dir / name
        if asset.get("state") != "uploaded":
            fail(f"RC asset {name} is not in uploaded state")
        if path.stat().st_size != asset.get("size"):
            fail(f"RC asset {name} size does not match GitHub metadata")
        expected = asset.get("digest")
        if not isinstance(expected, str) or not expected.startswith("sha256:"):
            fail(f"RC asset {name} has no GitHub sha256 digest")
        actual = sha256(path)
        if actual != expected.removeprefix("sha256:"):
            fail(f"RC asset {name} sha256 does not match GitHub metadata")
    return metadata


def validate_proof(path: pathlib.Path, rc_tag: str, commit_sha: str) -> str:
    values: dict[str, str] = {}
    for raw in path.read_text().splitlines():
        if "=" not in raw:
            fail(f"{path.name} contains malformed proof data")
        key, value = raw.split("=", 1)
        if key in values:
            fail(f"{path.name} contains duplicate {key}")
        values[key] = value
    if set(values) != {"tag", "run_id", "commit"}:
        fail(f"{path.name} does not contain exactly tag, run_id, and commit")
    if values["tag"] != rc_tag or values["commit"] != commit_sha:
        fail(f"{path.name} does not attest {rc_tag} at {commit_sha}")
    if not values["run_id"].isdigit():
        fail(f"{path.name} has an invalid run_id")
    return values["run_id"]


def validate_manifests(
    assets_dir: pathlib.Path,
    repository: str,
    rc_tag: str,
    stable_tag: str,
    version: str,
    asset_names: set[str],
) -> dict[str, dict[str, Any]]:
    source_prefix = f"https://github.com/{repository}/releases/download/{rc_tag}/"
    manifests: dict[str, dict[str, Any]] = {}

    for name, expected_platforms in MANIFEST_PLATFORMS.items():
        manifest = read_json(assets_dir / name)
        if not isinstance(manifest, dict) or manifest.get("version") != version:
            fail(f"{name} must be stamped with stable app version {version}")
        notes = manifest.get("notes") or ""
        if MINIMUM_LINE_RE.search(notes):
            fail(f"RC manifest {name} unexpectedly carries mutable Minimum-Version policy")
        platforms = manifest.get("platforms")
        if not isinstance(platforms, dict) or set(platforms) != expected_platforms:
            fail(f"{name} platform set is not exactly {sorted(expected_platforms)}")
        for platform, entry in platforms.items():
            if not isinstance(entry, dict) or not str(entry.get("signature") or "").strip():
                fail(f"{name} has no signature for {platform}")
            url = entry.get("url")
            if not isinstance(url, str) or not url.startswith(source_prefix):
                fail(f"{name} has an unexpected updater URL for {platform}: {url!r}")
            target = url.removeprefix(source_prefix)
            if target not in asset_names or "/" in target:
                fail(f"{name} points {platform} at missing RC asset {target!r}")
        manifests[name] = manifest

    combined: dict[str, Any] = {}
    for fragment_name in ("updater-aarch64-apple-darwin.json", "updater-windows.json"):
        combined.update(manifests[fragment_name]["platforms"])
    if manifests["latest.json"]["platforms"] != dict(sorted(combined.items())):
        fail("latest.json platform entries do not exactly match the signed fragments")

    stable_prefix = f"https://github.com/{repository}/releases/download/{stable_tag}/"
    for manifest in manifests.values():
        for entry in manifest["platforms"].values():
            entry["url"] = stable_prefix + entry["url"].removeprefix(source_prefix)
    return manifests


def stage(args: argparse.Namespace) -> None:
    match = RC_RE.fullmatch(args.rc_tag)
    if not match:
        fail("rc-tag must be canonical vX.Y.Z-rc.N")
    stable_tag = "v" + ".".join(match.groups()[:3])
    version = stable_tag[1:]
    if args.stable_tag != stable_tag:
        fail(f"stable tag must be {stable_tag}, derived from {args.rc_tag}")
    if not SHA_RE.fullmatch(args.commit_sha):
        fail("commit-sha must be a full lowercase commit SHA")
    if args.minimum_version and not VERSION_RE.fullmatch(args.minimum_version):
        fail("minimum-version must be canonical X.Y.Z")
    if args.minimum_version and tuple(map(int, args.minimum_version.split("."))) > tuple(map(int, version.split("."))):
        fail(f"minimum version {args.minimum_version} exceeds promoted version {version}")

    release = read_json(args.release_json)
    if release.get("tag_name") != args.rc_tag or release.get("draft") is not False or release.get("prerelease") is not True:
        fail(f"{args.rc_tag} must be an existing published prerelease")

    metadata = validate_source_files(args.assets_dir, release)
    run_ids = {
        validate_proof(args.assets_dir / "smoke-macos.ok", args.rc_tag, args.commit_sha),
        validate_proof(args.assets_dir / "smoke-windows.ok", args.rc_tag, args.commit_sha),
    }
    if len(run_ids) != 1:
        fail("macOS and Windows smoke proofs came from different release runs")

    manifests = validate_manifests(
        args.assets_dir,
        args.repository,
        args.rc_tag,
        args.stable_tag,
        version,
        set(metadata),
    )
    if args.minimum_version:
        notes = str(manifests["latest.json"].get("notes") or "")
        notes = MINIMUM_RE.sub("", notes).rstrip()
        manifests["latest.json"]["notes"] = (notes + "\n" if notes else "") + f"Minimum-Version: {args.minimum_version}"

    for name, manifest in manifests.items():
        (args.assets_dir / name).write_text(json.dumps(manifest, indent=2) + "\n")

    inventory = {
        "rc_tag": args.rc_tag,
        "stable_tag": args.stable_tag,
        "version": version,
        "commit_sha": args.commit_sha,
        "smoke_run_id": next(iter(run_ids)),
        "assets": [],
    }
    for name in sorted(metadata):
        path = args.assets_dir / name
        source_digest = metadata[name]["digest"].removeprefix("sha256:")
        staged_digest = sha256(path)
        changed = source_digest != staged_digest
        if changed != (name in MANIFEST_PLATFORMS):
            fail(f"{name} {'changed unexpectedly' if changed else 'was not rewritten as required'}")
        inventory["assets"].append(
            {
                "name": name,
                "size": path.stat().st_size,
                "sha256": staged_digest,
                "source_sha256": source_digest,
                "metadata_rewritten": changed,
            }
        )
    args.inventory.write_text(json.dumps(inventory, indent=2) + "\n")
    print(f"validated {args.rc_tag} and staged {len(metadata)} assets for {args.stable_tag}")


def verify(args: argparse.Namespace) -> None:
    inventory = read_json(args.inventory)
    release = read_json(args.release_json)
    if release.get("tag_name") != inventory.get("stable_tag"):
        fail("uploaded release tag does not match the promotion inventory")
    if release.get("draft") is not args.expect_draft or release.get("prerelease") is not False:
        state = "draft" if args.expect_draft else "published"
        fail(f"stable release is not the expected non-prerelease {state} release")
    actual = release_assets(release)
    expected = {asset["name"]: asset for asset in inventory["assets"]}
    if set(actual) != set(expected):
        fail("stable release asset names differ from the validated promotion inventory")
    for name, wanted in expected.items():
        got = actual[name]
        if got.get("state") != "uploaded" or got.get("size") != wanted["size"] or got.get("digest") != f"sha256:{wanted['sha256']}":
            fail(f"uploaded stable asset {name} does not match the staged sha256/size")
    print(f"verified {len(expected)} uploaded assets on {inventory['stable_tag']}")


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    sub = result.add_subparsers(dest="command", required=True)
    stage_parser = sub.add_parser("stage")
    stage_parser.add_argument("--assets-dir", type=pathlib.Path, required=True)
    stage_parser.add_argument("--release-json", type=pathlib.Path, required=True)
    stage_parser.add_argument("--inventory", type=pathlib.Path, required=True)
    stage_parser.add_argument("--repository", required=True)
    stage_parser.add_argument("--rc-tag", required=True)
    stage_parser.add_argument("--stable-tag", required=True)
    stage_parser.add_argument("--commit-sha", required=True)
    stage_parser.add_argument("--minimum-version", default="")
    stage_parser.set_defaults(func=stage)
    verify_parser = sub.add_parser("verify")
    verify_parser.add_argument("--release-json", type=pathlib.Path, required=True)
    verify_parser.add_argument("--inventory", type=pathlib.Path, required=True)
    verify_parser.add_argument("--expect-draft", action=argparse.BooleanOptionalAction, required=True)
    verify_parser.set_defaults(func=verify)
    return result


if __name__ == "__main__":
    parsed = parser().parse_args()
    parsed.func(parsed)
