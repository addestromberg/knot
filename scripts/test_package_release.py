import hashlib
import importlib.util
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
import zipfile

spec = importlib.util.spec_from_file_location("package_release", Path(__file__).with_name("package-release.py"))
packager = importlib.util.module_from_spec(spec)
spec.loader.exec_module(packager)


class Packages(unittest.TestCase):
    def test_archives_contain_both_executables_identity_and_matching_checksum(self):
        for target in ("x86_64-pc-windows-msvc", "x86_64-unknown-linux-gnu", "aarch64-apple-darwin"):
            with self.subTest(target=target), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                suffix = ".exe" if "windows" in target else ""
                names = [f"{name}{suffix}" for name in ("knot", "knot-mcp")]
                for name in names:
                    (root / name).write_bytes(f"executable {name}".encode())
                commit = "a" * 40
                documents = {"LICENSE": b"License terms", "THIRD-PARTY-NOTICES.txt": b"Dependency licenses"}
                archive = packager.package(root, root / "dist", target, "0.6.0", 12, commit, documents)
                self.assertIn("0.6.0-beta.12", archive.name)
                if suffix:
                    with zipfile.ZipFile(archive) as bundle:
                        self.assertEqual(set(bundle.namelist()), {*names, "BUILD-INFO.json", *documents})
                        for name in names:
                            self.assertEqual(bundle.read(name), (root / name).read_bytes())
                        identity = json.loads(bundle.read("BUILD-INFO.json"))
                        for name, contents in documents.items():
                            self.assertEqual(bundle.read(name), contents)
                else:
                    with tarfile.open(archive) as bundle:
                        self.assertEqual(set(bundle.getnames()), {*names, "BUILD-INFO.json", *documents})
                        for name in names:
                            self.assertEqual(bundle.getmember(name).mode, 0o755)
                            self.assertEqual(bundle.extractfile(name).read(), (root / name).read_bytes())
                        identity = json.load(bundle.extractfile("BUILD-INFO.json"))
                        for name, contents in documents.items():
                            self.assertEqual(bundle.extractfile(name).read(), contents)
                self.assertEqual(identity["commit"], commit)
                self.assertEqual(identity["target"], target)
                self.assertEqual(identity["version"], "0.6.0-beta.12")
                checksum = archive.with_name(archive.name + ".sha256").read_text()
                self.assertEqual(checksum, f"{hashlib.sha256(archive.read_bytes()).hexdigest()}  {archive.name}\n")

    def test_incomplete_build_cannot_produce_a_download(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "knot").write_bytes(b"executable")
            with self.assertRaisesRegex(ValueError, "knot-mcp"):
                packager.package(root, root / "dist", "x86_64-unknown-linux-gnu", "0.6.0", 1, "a" * 40, {})
            self.assertFalse((root / "dist").exists())


if __name__ == "__main__":
    unittest.main()
