"""Use synthetic ZIPs and tools to check the cache without running a build."""

import hashlib
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import DEFAULT, patch
from zipfile import ZipFile

import build_windows_candidate as candidate
import openrgb_cache as cache
from test_package_installer import amd64_fixture


class OpenRGBCacheTests(unittest.TestCase):
    def setUp(self):
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.root = Path(scratch.name)
        self.repo = self.root / "repo"
        self.component = self.repo / "components/openrgb"
        self.cache = self.root / "cache"
        self.source = "https://example.com/openrgb"
        self.revision = "a" * 40
        self.key = cache.KEY_PREFIX + "0" * 64

    def write(self, name, data):
        path = self.repo / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        return data

    def make_bundle(self, remove=None, change=None, linker=(14, 51)):
        def lock(name, value):
            return self.write("components/openrgb/" + name, json.dumps(value).encode())

        def pin(data):
            return {"sha256": hashlib.sha256(data).hexdigest(), "size": len(data)}

        files = {f"runtime/{name}": name.encode() for name in cache.REQUIRED}
        files["runtime/OpenRGB.exe"] = amd64_fixture(linker)
        files["build/source-lock.json"] = lock("source-lock.json", {
            "repository": self.source, "revision": self.revision, "patch": "test.patch",
        })
        files["build/dependency-lock.json"] = lock("dependency-lock.json", {
            "files": {"PawnIOLib.dll": pin(files["runtime/PawnIOLib.dll"])},
        })
        files["build/test.patch"] = self.write("components/openrgb/test.patch", b"fixture patch")
        for recipe in cache.RECIPES:
            files["build/" + recipe] = self.write("scripts/components/" + recipe, recipe.encode())
        files["build/VERIFICATION.md"] = self.write("docs/openrgb-component-verification.md", b"verification")
        assets = [f"licenses/{name}" for name in sorted(cache.LICENSES)] + ["source/fixture.zip"]
        files.update({name: name.encode() for name in assets})
        lock("distribution-assets.json", {"files": [{"path": name, **pin(files[name])} for name in assets]})
        self.write("scripts/installer/prerequisites.lock.json",
                   (cache.REPO / "scripts/installer/prerequisites.lock.json").read_bytes())
        if remove:
            del files[remove]
        if change:
            files[change] = b"changed but internally rehashed"
        manifest = {"version": 1, "scope": "packaging_only", "engine_repository": self.source,
                    "engine_revision": self.revision, "engine_sha256": pin(files["runtime/OpenRGB.exe"])["sha256"],
                    "files": [{"path": name, **pin(data)} for name, data in files.items()]}
        bundle = self.root / "component.zip"
        with ZipFile(bundle, "w") as archive:
            archive.writestr("component-manifest.json", json.dumps(manifest))
            for name, data in files.items():
                archive.writestr(name, data)
        return bundle

    def key_fixture(self):
        for path in cache.build_input_pins(cache.REPO):
            self.write(path, (cache.REPO / path).read_bytes())
        lock = "scripts/installer/prerequisites.lock.json"
        self.write(lock, (cache.REPO / lock).read_bytes())
        tool = self.root / "cl.exe"
        tool.write_bytes(b"fixture")
        return tool

    def test_key_changes_with_image_sdk_and_tool_bytes(self):
        tool = self.key_fixture()
        tools = {"cl": tool}
        baseline = cache.cache_key(self.repo, "win/1", "14.51/26100", tools)
        for image, toolchain in (("win/2", "14.51/26100"), ("win/1", "14.51/26101")):
            self.assertNotEqual(baseline, cache.cache_key(self.repo, image, toolchain, tools))
        tool.write_bytes(b"changed")
        self.assertNotEqual(baseline, cache.cache_key(self.repo, "win/1", "14.51/26100", tools))

    def test_key_ignores_general_workflow_packaging_and_other_prerequisite_pins(self):
        tool = self.key_fixture()
        files = (".github/workflows/windows.yml", "scripts/build_windows_candidate.py",
                 "scripts/package_installer.py", "scripts/package_release.py")
        for path in files:
            self.write(path, b"before")
        key = lambda: cache.cache_key(self.repo, "win/1", "14.51/26100", {"cl": tool})
        baseline = key()
        for path in files:
            self.write(path, b"after")
        lock = "scripts/installer/prerequisites.lock.json"
        pins = json.loads((self.repo / lock).read_bytes())
        pins["packages"]["webview2_x64"]["sha256"] = "b" * 64
        self.write(lock, json.dumps(pins).encode())
        self.assertEqual(key(), baseline)
        pins["minimum_vc_runtime"] = "14.50.0.0"
        self.write(lock, json.dumps(pins).encode())
        self.assertNotEqual(key(), baseline)

    def test_key_covers_openrgb_sources_pinned_inputs_recipes_and_verifier(self):
        tool = self.key_fixture()
        key = lambda: cache.cache_key(self.repo, "win/1", "14.51/26100", {"cl": tool})
        baseline = key()
        paths = (
            "scripts/build_openrgb_component.py", "scripts/pinned_setup_download.py",
            "scripts/components/openrgb_cache_key.ps1", "scripts/components/build_openrgb.ps1",
            "scripts/components/test_verify_openrgb_component.py", "scripts/component_archive.py",
            "components/openrgb/source-lock.json", "components/openrgb/dependency-lock.json",
            "components/openrgb/distribution-assets.json", "components/openrgb/prismestra-pc.patch",
            "components/openrgb/licenses/OpenRGB-GPL-2.0.txt", "docs/openrgb-component-verification.md",
        )
        for path in paths:
            with self.subTest(path=path):
                original = (self.repo / path).read_bytes()
                self.write(path, original + b"\n")
                self.assertNotEqual(key(), baseline)
                self.write(path, original)

    def test_complete_component_round_trip_and_exact_key(self):
        original = self.make_bundle()
        cache.save(self.cache, self.key, original)
        restored = cache.restore(self.cache, self.key, self.root, self.repo)
        self.assertEqual(restored.read_bytes(), original.read_bytes())
        with self.assertRaisesRegex(ValueError, "exact build key"):
            cache.restore(self.cache, self.key + "x", self.root, self.repo)
        with restored.open("ab") as stream:
            stream.write(b"altered")
        with self.assertRaisesRegex(ValueError, "unpinned file"):
            cache.restore(self.cache, self.key, self.root, self.repo)

    def test_self_consistent_zip_cannot_omit_a_pinned_source(self):
        cache.save(self.cache, self.key, self.make_bundle(remove="source/fixture.zip"))
        with self.assertRaisesRegex(ValueError, "incomplete"):
            cache.restore(self.cache, self.key, self.root, self.repo)

    def test_self_consistent_zip_cannot_change_a_pinned_dependency(self):
        cache.save(self.cache, self.key, self.make_bundle(change="runtime/PawnIOLib.dll"))
        with self.assertRaisesRegex(ValueError, "dependency differs"):
            cache.restore(self.cache, self.key, self.root, self.repo)

    def test_restored_engine_must_fit_the_runtime_floor(self):
        cache.save(self.cache, self.key, self.make_bundle(linker=(14, 52)))
        with self.assertRaisesRegex(ValueError, "exceeds minimum VC runtime"):
            cache.restore(self.cache, self.key, self.root, self.repo)

    def test_candidate_uses_the_cold_path_for_a_bad_cache(self):
        original = self.make_bundle()
        cache.save(self.cache, self.key, original)
        with (self.cache / cache.BUNDLE).open("ab") as stream:
            stream.write(b"altered")
        with patch.object(candidate, "build_component", return_value=original) as build:
            result = candidate.load_or_build_component(self.root, self.root / "vs.bat", self.cache, self.key)
        self.assertEqual(result, original)
        build.assert_called_once_with(self.root, self.root / "vs.bat")
        receipt = json.loads((self.cache / cache.RECEIPT).read_bytes())
        self.assertEqual(receipt["key"], self.key)
        self.assertFalse((self.root / cache.READY).exists())

    def test_candidate_passes_the_exact_cli_cache_key_after_fetching_prerequisites(self):
        output = self.root / "output"
        vsdevcmd = self.root / "VsDevCmd.bat"
        vsdevcmd.touch()
        argv = ["build_windows_candidate.py", "--commit", "a" * 40, "--version", "0.1.0.8",
                "--output", str(output), "--vsdevcmd", str(vsdevcmd),
                "--openrgb-cache", str(self.cache), "--openrgb-cache-key", self.key]
        with (
            patch.object(sys, "argv", argv),
            patch("package_release.clean_commit"),
            patch.multiple(candidate, fetch=DEFAULT, checkout=DEFAULT, run=DEFAULT,
                           load_or_build_component=DEFAULT, package_candidate=DEFAULT) as stages,
        ):
            candidate.main()
        stages["load_or_build_component"].assert_called_once_with(
            output.resolve(), vsdevcmd.resolve(), self.cache, self.key)

    def test_candidate_only_queues_a_new_cache_after_a_cold_build(self):
        original = self.make_bundle()
        vsdevcmd = self.root / "vs.bat"
        with patch.object(candidate, "build_component", return_value=original) as build:
            result = candidate.load_or_build_component(self.root, vsdevcmd, self.cache, self.key)
        build.assert_called_once_with(self.root, vsdevcmd)
        self.assertEqual(result, original)
        self.assertTrue((self.root / cache.READY).is_file())
        self.assertEqual(cache.restore(self.cache, self.key, self.root, self.repo).read_bytes(), original.read_bytes())

    def test_candidate_skips_the_build_after_validated_restore(self):
        original = self.make_bundle()
        cache.save(self.cache, self.key, original)
        restore = cache.restore
        with (
            patch.object(cache, "restore", side_effect=lambda *args: restore(*args, repo=self.repo)),
            patch.object(candidate, "build_component") as build,
        ):
            result = candidate.load_or_build_component(self.root, self.root / "vs.bat", self.cache, self.key)
        build.assert_not_called()
        self.assertEqual(result.read_bytes(), original.read_bytes())
        self.assertFalse((self.root / cache.READY).exists())


if __name__ == "__main__":
    unittest.main()
