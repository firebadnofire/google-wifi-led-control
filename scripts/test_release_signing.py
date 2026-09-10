#!/usr/bin/env python3
"""Exercise the workflow's actual signing block with disposable key material."""

import base64
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import textwrap
import unittest


ROOT = Path(__file__).resolve().parents[1]


@unittest.skipUnless(shutil.which("gpg") and shutil.which("bash"), "Bash and GPG required")
class ReleaseSigningTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="gale-signing-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.workspace = self.root / "workspace"
        self.release = self.workspace / "release"
        self.release.mkdir(parents=True)
        for name in ("gale-led-test.apk", "luci-app-gale-led-test.apk"):
            (self.release / name).write_bytes(b"Disposable signing fixture\n")
        self.runtime = self.root / "runtime"
        self.runtime.mkdir()
        source = (ROOT / ".forgejo/workflows/release.yml").read_text()
        marker = "      - name: Sign packages and checksums\n        run: |\n"
        self.script = textwrap.dedent(source.split(marker, 1)[1].split("\n      - name:", 1)[0])

    def sign(self, encoded):
        environment = {
            **os.environ,
            "LOCAL_WORKSPACE": str(self.workspace),
            "TMPDIR": str(self.runtime),
            "CI_KEY": encoded,
            "CI_KEY_PASSPHRASE": "disposable-test-passphrase",
        }
        result = subprocess.run(
            ["bash", "-c", self.script], env=environment,
            capture_output=True, text=True, timeout=60)
        self.assertEqual(list(self.runtime.iterdir()), [], "Signing state was not cleaned up")
        self.assertNotIn(encoded, result.stdout + result.stderr)
        return result

    def test_invalid_base64_stops_before_signing(self):
        result = self.sign("this-is-not-base64!")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("CI_KEY must be a valid base64-encoded", result.stderr)
        self.assertFalse((self.release / "SHA256SUMS").exists())
        self.assertEqual(list(self.release.glob("*.asc")), [])

    def test_base64_without_a_key_stops_before_signing(self):
        result = self.sign(base64.b64encode(b"not OpenPGP data").decode())
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.release / "SHA256SUMS").exists())
        self.assertEqual(list(self.release.glob("*.asc")), [])

    def test_base64_key_signs_and_verifies_all_artifacts(self):
        key_home = self.root / "key-generation"
        key_home.mkdir(mode=0o700)
        common = ["gpg", "--homedir", str(key_home), "--batch", "--pinentry-mode", "loopback",
                  "--passphrase", "disposable-test-passphrase"]
        subprocess.run(
            [*common, "--quick-generate-key", "Disposable CI test <ci@example.invalid>",
             "ed25519", "sign", "1d"], check=True, capture_output=True, timeout=60)
        exported = subprocess.run(
            [*common, "--armor", "--export-secret-keys"],
            check=True, capture_output=True, timeout=60).stdout
        self.assertTrue(exported)
        result = self.sign(base64.b64encode(exported).decode())
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(list(self.release.glob("*.asc"))), 3)
        subprocess.run(
            ["sha256sum", "--check", "--strict", "SHA256SUMS"], cwd=self.release,
            check=True, capture_output=True)


if __name__ == "__main__":
    unittest.main()
