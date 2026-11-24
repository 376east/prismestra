"""Small synthetic packaging inventories only; no binaries or hardware execute."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import zipfile

import package_openrgb_component as packager


class PackagingChecks(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="openrgb-package-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.args = argparse.Namespace(**{name: self.root / name for name in (
            "runtime_dir", "source_archives_dir", "licenses_dir", "recipes_dir", "source_lock",
            "dependency_lock", "patch", "inventory", "asset_manifest", "verification_doc")})
        self.args.output = self.root / "component.zip"
        for name in ("runtime_dir", "source_archives_dir", "licenses_dir", "recipes_dir"):
            getattr(self.args, name).mkdir()
        self.args.patch.write_bytes(b"synthetic derivative patch")
        self.args.verification_doc.write_bytes(b"No legal or hardware acceptance inferred.")
        runtime = []
        for name in sorted(packager.verifier.REQUIRED):
            path = self.args.runtime_dir / name
            path.write_bytes(b"not executable: " + name.encode())
            runtime.append({"name": name, **self.info(path)})
        source_names = [f"openrgb-headless-{packager.verifier.SOURCE_HEAD}-source.zip",
                        "hidapi-" + "2" * 40 + "-source.zip", "PawnIO-" + "3" * 40 + "-source.zip",
                        "PawnIO.Modules-" + "4" * 40 + "-source.zip", "libusb-1.0.27-source.tar.bz2"]
        assets = []
        for role, directory, names in (("source", self.args.source_archives_dir, source_names),
                                       ("licenses", self.args.licenses_dir, sorted(packager.LICENSES))):
            for name in names:
                path = directory / name
                path.write_bytes(b"synthetic pinned asset: " + name.encode())
                assets.append({"path": role + "/" + name, **self.info(path)})
        for name in packager.RECIPES:
            path = self.args.recipes_dir / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b"synthetic recipe: " + name.encode())
        self.write(self.args.source_lock, {"version": 1, "repository": "https://example.com/component",
                   "revision": packager.verifier.SOURCE_HEAD, "patch": "component.patch",
                   "patch_sha256": self.info(self.args.patch)["sha256"], "untracked_source_paths": []})
        self.write(self.args.dependency_lock, {"version": 1, "files": {
            name: {"sha256": self.info(self.args.runtime_dir / name)["sha256"],
                   "source_revision": revision * 40,
                   "source_archive_sha256": self.info(self.args.source_archives_dir / archive)["sha256"]}
            for name, archive, revision in (("hidapi-hotplug.dll", source_names[1], "2"),
                                            ("PawnIOLib.dll", source_names[2], "3"))}})
        self.write(self.args.inventory, {"schema_version": 1, "scope": "static_component_candidate",
                   "ok": True, "errors": [], "source_head": packager.verifier.SOURCE_HEAD,
                   "source_patch_locked": True, "untracked_paths": [],
                   "tracked_patch": self.info(self.args.patch), "files": runtime})
        self.write(self.args.asset_manifest, {"version": 1, "files": assets})

    @staticmethod
    def info(path):
        data = path.read_bytes()
        return {"size": len(data), "sha256": hashlib.sha256(data).hexdigest()}

    @staticmethod
    def write(path, value):
        path.write_text(json.dumps(value), encoding="utf-8")

    def mutate(self, path, function):
        value = json.loads(path.read_text())
        function(value)
        self.write(path, value)

    def test_cli_creates_complete_hashed_bundle_without_machine_paths_or_execution_claim(self):
        command = [sys.executable, str(Path(packager.__file__))]
        for name, value in vars(self.args).items():
            command.extend(["--" + name.replace("_", "-"), str(value)])
        completed = subprocess.run(command, capture_output=True, text=True, check=True)
        report = json.loads(completed.stdout)
        self.assertEqual(report["crc_and_manifest"], "PASS")
        self.assertFalse(report["native_hardware_activated"])
        with zipfile.ZipFile(self.args.output) as archive:
            self.assertIsNone(archive.testzip())
            manifest = json.loads(archive.read("component-manifest.json"))
            self.assertEqual(set(archive.namelist()), {i["path"] for i in manifest["files"]} | {"component-manifest.json"})
            self.assertIn("build/pawniolib/CMakeLists.txt", archive.namelist())
            self.assertEqual(len([i for i in manifest["files"] if i["path"].startswith("source/")]), 5)
            self.assertIn("license approval", manifest["unverified"])
            self.assertNotIn(str(self.root), json.dumps(manifest))
            self.assertNotIn("product_commit", manifest)
            for item in manifest["files"]:
                data = archive.read(item["path"])
                self.assertEqual(self.info_bytes(data), {k: item[k] for k in ("size", "sha256")})

    @staticmethod
    def info_bytes(data):
        return {"size": len(data), "sha256": hashlib.sha256(data).hexdigest()}

    def test_wrong_runtime_patch_or_asset_hash_is_rejected_before_output(self):
        for path in (self.args.runtime_dir / "OpenRGB.exe", self.args.patch,
                     next(self.args.source_archives_dir.iterdir()), self.args.licenses_dir / "PawnIO-COPYING"):
            original = path.read_bytes()
            path.write_bytes(original + b"changed")
            with self.assertRaises(ValueError):
                packager.package(self.args)
            self.assertFalse(self.args.output.exists())
            path.write_bytes(original)

    def test_patched_hidapi_requires_overlay_and_rebuilt_dll(self):
        overlay = self.root / "hidapi-teardown.patch"
        overlay.write_bytes(b"fixture HIDAPI patch")
        dll = self.args.runtime_dir / "hidapi-hotplug.dll"
        old_dll = dll.read_bytes()
        dll.write_bytes(b"fixture rebuilt DLL")
        self.mutate(self.args.dependency_lock, lambda data: data["files"][dll.name].update(
            patch=overlay.name, patch_sha256=self.info(overlay)["sha256"], sha256=self.info(dll)["sha256"]))
        original_overlay = overlay.read_bytes()
        overlay.unlink()
        with self.assertRaises(FileNotFoundError):
            packager.package(self.args)
        overlay.write_bytes(b"altered patch")
        with self.assertRaisesRegex(ValueError, "HIDAPI patch SHA256 mismatch"):
            packager.package(self.args)
        overlay.write_bytes(original_overlay)

        # A self-consistent prior inventory still cannot authorize Preview DLL bytes.
        dll.write_bytes(old_dll)
        with self.assertRaisesRegex(ValueError, "rebuilt dependency binary/source pins mismatch"):
            packager.package(self.args)
        self.assertFalse(self.args.output.exists())

        dll.write_bytes(b"fixture rebuilt DLL")
        self.mutate(self.args.inventory, lambda data: next(
            entry for entry in data["files"] if entry["name"] == dll.name).update(self.info(dll)))
        packager.package(self.args)
        with zipfile.ZipFile(self.args.output) as archive:
            self.assertEqual(archive.read("build/" + overlay.name), original_overlay)

    def test_missing_required_sources_notices_and_recipes_is_rejected(self):
        for path in (next(self.args.source_archives_dir.iterdir()), self.args.licenses_dir / "PawnIO-COPYING",
                     self.args.recipes_dir / "pawniolib/CMakeLists.txt"):
            original = path.read_bytes()
            path.unlink()
            with self.assertRaises((OSError, ValueError)):
                packager.package(self.args)
            self.assertFalse(self.args.output.exists())
            path.write_bytes(original)

    def test_manifest_escape_case_collision_and_wrong_source_pin_are_rejected(self):
        original = self.args.asset_manifest.read_bytes()
        for name in ("../outside", "/absolute", "source/C:/escape", "source/CON", "source/file."):
            self.mutate(self.args.asset_manifest, lambda data: data["files"][0].update(path=name))
            with self.assertRaises(ValueError):
                packager.package(self.args)
            self.args.asset_manifest.write_bytes(original)
        self.mutate(self.args.asset_manifest, lambda data: data["files"].append(
            {**data["files"][0], "path": data["files"][0]["path"].upper()}))
        with self.assertRaises(ValueError):
            packager.package(self.args)
        self.args.asset_manifest.write_bytes(original)
        self.mutate(self.args.dependency_lock, lambda data: data["files"]["PawnIOLib.dll"].update(source_archive_sha256="0" * 64))
        with self.assertRaises(ValueError):
            packager.package(self.args)
        self.assertFalse(self.args.output.exists())

    def test_failed_static_gate_and_runtime_extras_cannot_be_packaged(self):
        self.mutate(self.args.inventory, lambda data: data.update(ok=False))
        with self.assertRaises(ValueError):
            packager.package(self.args)
        self.mutate(self.args.inventory, lambda data: data.update(ok=True))
        (self.args.runtime_dir / "unexpected.dll").write_bytes(b"extra")
        with self.assertRaises(ValueError):
            packager.package(self.args)

    def test_output_is_exclusive_and_cannot_modify_input_tree(self):
        self.args.output.write_bytes(b"keep existing artifact")
        with self.assertRaises(ValueError):
            packager.package(self.args)
        self.assertEqual(self.args.output.read_bytes(), b"keep existing artifact")
        self.args.output = self.args.source_archives_dir / "new.zip"
        with self.assertRaises(ValueError):
            packager.package(self.args)
        self.assertFalse(self.args.output.exists())

    def test_symlink_ancestor_is_rejected(self):
        link = self.root / "linked-licenses"
        try:
            link.symlink_to(self.args.licenses_dir, target_is_directory=True)
        except OSError as error:
            self.skipTest("symlink creation unavailable: " + str(error))
        self.args.licenses_dir = link
        with self.assertRaisesRegex(ValueError, "symlink/reparse"):
            packager.package(self.args)
        self.assertFalse(self.args.output.exists())


class RepositoryPinsChecks(unittest.TestCase):
    def test_real_notices_and_patch_survive_checkout_with_their_pinned_bytes(self):
        root = Path(__file__).resolve().parents[2]
        component = root / "components/openrgb"
        manifest = json.loads((component / "distribution-assets.json").read_text(encoding="utf-8"))
        notices = [item for item in manifest["files"] if item["path"].startswith("licenses/")]
        self.assertEqual({Path(item["path"]).name for item in notices}, packager.LICENSES)
        for item in notices:
            data = (component / item["path"]).read_bytes()
            self.assertEqual(len(data), item["size"], item["path"])
            self.assertEqual(hashlib.sha256(data).hexdigest(), item["sha256"], item["path"])
        lock = json.loads((component / "source-lock.json").read_text(encoding="utf-8"))
        self.assertEqual(hashlib.sha256((component / lock["patch"]).read_bytes()).hexdigest(), lock["patch_sha256"])


if __name__ == "__main__":
    unittest.main()
