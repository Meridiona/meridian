import hashlib
import json
import pathlib
import subprocess
import sys
import tempfile
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "promote-rc-assets.py"
RC_TAG = "v1.92.3-rc.2"
STABLE_TAG = "v1.92.3"
COMMIT = "97feed114cffd02c349502b1f1129ebb9f3af6f7"
REPOSITORY = "Meridiona/meridian"


class PromoteRcAssetsTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.temp.name)
        self.assets = self.root / "assets"
        self.assets.mkdir()
        prefix = f"https://github.com/{REPOSITORY}/releases/download/{RC_TAG}/"
        mac = {
            "version": "1.92.3",
            "notes": "Meridian v1.92.3",
            "pub_date": "2026-10-02T04:57:08Z",
            "platforms": {"darwin-aarch64": {"signature": "mac-signature", "url": prefix + "Meridian-aarch64.app.tar.gz"}},
        }
        windows = {
            "version": "1.92.3",
            "notes": "Meridian v1.92.3",
            "pub_date": "2026-10-02T04:21:40Z",
            "platforms": {
                "windows-x86_64": {"signature": "win-signature", "url": prefix + "Meridian-x86_64-setup.exe"},
                "windows-x86_64-nsis": {"signature": "win-signature", "url": prefix + "Meridian-x86_64-setup.exe"},
            },
        }
        latest = {
            "version": "1.92.3",
            "notes": "Meridian v1.92.3",
            "pub_date": "2026-10-02T04:57:38Z",
            "platforms": dict(sorted((mac["platforms"] | windows["platforms"]).items())),
        }
        files = {
            "Meridian-aarch64.app.tar.gz": b"proven mac app",
            "Meridian-aarch64.app.tar.gz.sig": b"mac detached signature",
            "Meridian-aarch64.dmg": b"notarized dmg",
            "Meridian-x86_64-setup.exe": b"proven windows installer",
            "Meridian-x86_64-setup.exe.sig": b"windows detached signature",
            "smoke-macos.ok": f"tag={RC_TAG}\nrun_id=36963425800\ncommit={COMMIT}\n".encode(),
            "smoke-windows.ok": f"tag={RC_TAG}\r\nrun_id=36963425800\r\ncommit={COMMIT}\r\n".encode(),
            "updater-aarch64-apple-darwin.json": json.dumps(mac).encode(),
            "updater-windows.json": json.dumps(windows).encode(),
            "latest.json": json.dumps(latest).encode(),
        }
        for name, content in files.items():
            (self.assets / name).write_bytes(content)
        self.release = self.root / "release.json"
        self.inventory = self.root / "inventory.json"
        self.write_release(self.release, RC_TAG, draft=False, prerelease=True)

    def tearDown(self):
        self.temp.cleanup()

    def digest(self, path):
        return hashlib.sha256(path.read_bytes()).hexdigest()

    def write_release(self, path, tag, draft, prerelease, inventory=None):
        if inventory is None:
            assets = [
                {"name": item.name, "state": "uploaded", "size": item.stat().st_size, "digest": f"sha256:{self.digest(item)}"}
                for item in sorted(self.assets.iterdir())
            ]
        else:
            assets = [
                {"name": item["name"], "state": "uploaded", "size": item["size"], "digest": f"sha256:{item['sha256']}"}
                for item in inventory["assets"]
            ]
        path.write_text(json.dumps({"tag_name": tag, "draft": draft, "prerelease": prerelease, "assets": assets}))

    def stage(self, *extra):
        return subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "stage",
                "--assets-dir",
                str(self.assets),
                "--release-json",
                str(self.release),
                "--inventory",
                str(self.inventory),
                "--repository",
                REPOSITORY,
                "--rc-tag",
                RC_TAG,
                "--stable-tag",
                STABLE_TAG,
                "--commit-sha",
                COMMIT,
                *extra,
            ],
            text=True,
            capture_output=True,
        )

    def test_stages_only_updater_metadata_and_verifies_upload(self):
        binary_digest = self.digest(self.assets / "Meridian-aarch64.dmg")
        result = self.stage("--minimum-version", "1.92.2")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.digest(self.assets / "Meridian-aarch64.dmg"), binary_digest)

        latest = json.loads((self.assets / "latest.json").read_text())
        self.assertIn("Minimum-Version: 1.92.2", latest["notes"])
        for entry in latest["platforms"].values():
            self.assertIn(f"/download/{STABLE_TAG}/", entry["url"])

        inventory = json.loads(self.inventory.read_text())
        changed = {item["name"] for item in inventory["assets"] if item["metadata_rewritten"]}
        self.assertEqual(changed, {"latest.json", "updater-aarch64-apple-darwin.json", "updater-windows.json"})

        stable = self.root / "stable.json"
        self.write_release(stable, STABLE_TAG, draft=True, prerelease=False, inventory=inventory)
        verify = subprocess.run(
            [sys.executable, str(SCRIPT), "verify", "--release-json", str(stable), "--inventory", str(self.inventory), "--expect-draft"],
            text=True,
            capture_output=True,
        )
        self.assertEqual(verify.returncode, 0, verify.stderr)

    def test_fails_closed_when_downloaded_binary_does_not_match_github_digest(self):
        path = self.assets / "Meridian-aarch64.dmg"
        path.write_bytes(b"x" * path.stat().st_size)
        result = self.stage()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("sha256 does not match", result.stderr)

    def test_fails_closed_when_smoke_proof_names_another_commit(self):
        proof = self.assets / "smoke-macos.ok"
        proof.write_text(f"tag={RC_TAG}\nrun_id=36963425800\ncommit={'0' * 40}\n")
        self.write_release(self.release, RC_TAG, draft=False, prerelease=True)
        result = self.stage()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("does not attest", result.stderr)

    def test_reads_known_legacy_newline_marker_and_rejects_other_trailing_data(self):
        manifest = self.root / "previous-latest.json"
        manifest.write_text(json.dumps({"notes": "Meridian v1.92.2\nMinimum-Version: 1.92.0"}) + r"\n")
        accepted = subprocess.run(
            [sys.executable, str(SCRIPT), "minimum", "--manifest", str(manifest)],
            text=True,
            capture_output=True,
        )
        self.assertEqual(accepted.returncode, 0, accepted.stderr)
        self.assertEqual(accepted.stdout.strip(), "1.92.0")
        self.assertIn("legacy trailing", accepted.stderr)

        manifest.write_text(json.dumps({"notes": "Minimum-Version: 1.92.0"}) + " garbage")
        rejected = subprocess.run(
            [sys.executable, str(SCRIPT), "minimum", "--manifest", str(manifest)],
            text=True,
            capture_output=True,
        )
        self.assertNotEqual(rejected.returncode, 0)
        self.assertIn("cannot read valid JSON", rejected.stderr)


if __name__ == "__main__":
    unittest.main()
