"""Use synthetic ZIPs and tools to check the cache without running a build."""

import hashlib
import io
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import DEFAULT, call, patch
from zipfile import ZipFile

import build_openrgb_component as builder
import build_windows_candidate as candidate
import openrgb_cache as cache
from components import verify_openrgb_component as verifier
from components import build_hidapi
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

    def test_patched_hidapi_staging_requires_exact_build_tools_and_dll(self):
        runtime = self.root / "runtime"
        runtime.mkdir()
        dll = runtime / "hidapi-hotplug.dll"
        dll.write_bytes(b"old baseline DLL")
        archive = self.root / "archives" / f"hidapi-hotplug-{'a' * 40}-source.zip"
        archive.parent.mkdir()
        with ZipFile(archive, "w") as source:
            source.writestr(f"hidapi-hotplug-{'a' * 40}/windows/hid.c", b"pinned fixture source")
        self.component.mkdir(parents=True)
        overlay = self.component / "hidapi-teardown.patch"
        overlay.write_bytes(b"pinned fixture patch")
        lock = self.component / "dependency-lock.json"
        candidate = b"expected rebuilt DLL"
        pins = {
            "source_revision": "a" * 40, "source_archive_sha256": build_hidapi.sha256(archive),
            "patch": overlay.name, "patch_sha256": build_hidapi.sha256(overlay),
            "msvc_toolset": "14.44.35207", "windows_sdk": "10.0.26100.0",
            "sha256": hashlib.sha256(candidate).hexdigest(),
        }
        dependencies = {"files": {dll.name: pins}}
        devcmd = self.root / "VS/Common7/Tools/VsDevCmd.bat"
        env = {"VCTOOLSVERSION": pins["msvc_toolset"], "WINDOWSSDKVERSION": pins["windows_sdk"] + "\\"}

        def fake_build(args, **kwargs):
            if args[1] == "--build":
                result = args[2] / "src/windows/hidapi-hotplug.dll"
                result.parent.mkdir(parents=True)
                result.write_bytes(output_bytes)

        def stage(work):
            builder.stage_pinned_hidapi(runtime, archive.parent, lock, dependencies, work, devcmd)

        with patch.object(build_hidapi, "msvc_environment", return_value={**env, "VCTOOLSVERSION": "15"}):
            with self.assertRaisesRegex(ValueError, "toolchain differs"):
                stage(self.root / "drifted-toolset")
        self.assertEqual(dll.read_bytes(), b"old baseline DLL")
        with patch.object(build_hidapi, "msvc_environment", return_value=env), \
             patch.object(build_hidapi.subprocess, "run", side_effect=fake_build) as run:
            output_bytes = b"old baseline DLL"
            with self.assertRaisesRegex(ValueError, "DLL differs"):
                stage(self.root / "drifted-dll")
            self.assertEqual(dll.read_bytes(), b"old baseline DLL")
            output_bytes = candidate
            stage(self.root / "rebuilt-dll")
            self.assertTrue(any("-DCMAKE_SHARED_LINKER_FLAGS=/Brepro" in invocation.args[0]
                                for invocation in run.call_args_list))
        self.assertEqual(dll.read_bytes(), candidate)

    def make_bundle(self, remove=None, change=None, linker=(14, 51), patched_hidapi=False):
        def lock(name, value):
            return self.write("components/openrgb/" + name, json.dumps(value).encode())

        def pin(data):
            return {"sha256": hashlib.sha256(data).hexdigest(), "size": len(data)}

        files = {f"runtime/{name}": name.encode() for name in cache.REQUIRED}
        files["runtime/OpenRGB.exe"] = amd64_fixture(linker)
        patch_bytes = self.write("components/openrgb/test.patch", b"fixture patch")
        files["build/source-lock.json"] = lock("source-lock.json", {
            "repository": self.source, "revision": self.revision, "patch": "test.patch",
            "patch_sha256": pin(patch_bytes)["sha256"],
        })
        dependencies = {"PawnIOLib.dll": pin(files["runtime/PawnIOLib.dll"])}
        if patched_hidapi:
            overlay = self.write("components/openrgb/hidapi-teardown.patch", b"fixture HIDAPI patch")
            files["build/hidapi-teardown.patch"] = overlay
            files["runtime/hidapi-hotplug.dll"] = b"fixture rebuilt DLL"
            dependencies["hidapi-hotplug.dll"] = {
                **pin(files["runtime/hidapi-hotplug.dll"]),
                "patch": "hidapi-teardown.patch", "patch_sha256": pin(overlay)["sha256"],
            }
        files["build/dependency-lock.json"] = lock("dependency-lock.json", {"files": dependencies})
        files["build/test.patch"] = patch_bytes
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

    def test_old_cache_with_same_engine_revision_cannot_supply_new_dependency(self):
        cache.save(self.cache, self.key, self.make_bundle())
        self.make_bundle(patched_hidapi=True)
        with self.assertRaisesRegex(ValueError, "incomplete"):
            cache.restore(self.cache, self.key, self.root, self.repo)

    def test_cache_requires_exact_hidapi_overlay_and_rebuilt_dll(self):
        for change, remove, error in (
            (None, "build/hidapi-teardown.patch", "incomplete"),
            ("build/hidapi-teardown.patch", None, "member differs"),
            ("runtime/hidapi-hotplug.dll", None, "dependency differs"),
        ):
            with self.subTest(change=change, remove=remove):
                bundle = self.make_bundle(change=change, remove=remove, patched_hidapi=True)
                location = self.root / ("cache-" + error.replace(" ", "-"))
                cache.save(location, self.key, bundle)
                with self.assertRaisesRegex(ValueError, error):
                    cache.restore(location, self.key, self.root, self.repo)

    def test_hidapi_overlay_cannot_shadow_the_openrgb_patch(self):
        bundle = self.make_bundle(patched_hidapi=True)
        with ZipFile(bundle) as archive:
            manifest = json.loads(archive.read("component-manifest.json"))
        path = self.component / "dependency-lock.json"
        dependencies = json.loads(path.read_bytes())
        dependencies["files"]["hidapi-hotplug.dll"].update(
            patch="test.patch", patch_sha256=verifier.fingerprint(self.component / "test.patch")["sha256"],
        )
        path.write_text(json.dumps(dependencies), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "collides with an existing build member"):
            cache.verify_members(manifest, self.repo)

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
            output.resolve(), vsdevcmd.resolve(), self.cache, self.key, write_cache=False)

    def test_openrgb_only_verifies_new_cache_without_entering_setup_packaging(self):
        source = self.make_bundle()
        output = self.root / "output"
        vsdevcmd = self.root / "VsDevCmd.bat"
        vsdevcmd.touch()
        argv = ["build_windows_candidate.py", "--commit", "a" * 40, "--openrgb-only",
                "--output", str(output), "--vsdevcmd", str(vsdevcmd),
                "--openrgb-cache", str(self.cache), "--openrgb-cache-key", self.key, "--write-openrgb-cache"]
        restore = cache.restore
        with (
            patch.object(sys, "argv", argv),
            patch("package_release.clean_commit"),
            patch.object(cache, "restore", side_effect=lambda *args: restore(*args, repo=self.repo)) as verify,
            patch.multiple(candidate, fetch=DEFAULT, checkout=DEFAULT, run=DEFAULT,
                           build_component=DEFAULT, package_candidate=DEFAULT) as stages,
        ):
            stages["build_component"].return_value = source
            candidate.main()
        stages["build_component"].assert_called_once_with(output.resolve(), vsdevcmd.resolve())
        verify.assert_called_once_with(self.cache, self.key, output.resolve())
        for name in ("fetch", "checkout", "run", "package_candidate"):
            stages[name].assert_not_called()
        self.assertTrue((output / cache.READY).is_file())

    def test_read_only_cache_miss_builds_without_staging_a_cache_entry(self):
        original = self.make_bundle()
        with patch.object(candidate, "build_component", return_value=original) as build:
            result = candidate.load_or_build_component(self.root, self.root / "vs.bat", self.cache, self.key)
        build.assert_called_once()
        self.assertEqual(result, original)
        self.assertFalse(self.cache.exists())
        self.assertFalse((self.root / cache.READY).exists())

    def test_candidate_only_queues_a_new_cache_after_a_cold_build(self):
        original = self.make_bundle()
        downloads = self.root / "downloads"
        downloads.mkdir()
        previous_component = io.BytesIO()
        with ZipFile(previous_component, "w") as archive:
            archive.writestr("source/fixture.zip", b"source/fixture.zip")
        with ZipFile(downloads / "Prismestra-Sources-0.1.0.5.zip", "w") as archive:
            archive.writestr("openrgb/Prismestra-OpenRGB-component.zip", previous_component.getvalue())
        with ZipFile(downloads / "jom.zip", "w") as archive:
            archive.writestr("jom.exe", b"fixture")

        source = self.root / "openrgb-source"
        patch_path = self.component / "test.patch"
        locked_digest = builder.read(self.component / "source-lock.json")["patch_sha256"]
        sha256 = builder.sha256
        vsdevcmd = self.root / "vs.bat"
        calls = []

        def run(exe, *args):
            if exe == "git":
                calls.append(call(exe, *args))
            elif exe.name == "7z.exe":
                qmake = self.root / "qt/bin/qmake.exe"
                qmake.parent.mkdir(parents=True)
                qmake.touch()
            elif exe == builder.POWERSHELL:
                exe_file = self.root / "openrgb-build/release/OpenRGB.exe"
                exe_file.parent.mkdir(parents=True)
                exe_file.write_bytes(amd64_fixture())
            else:
                self.fail(f"unexpected subprocess: {exe}")

        def digest(path):
            if path == self.root / "previous-component.zip":
                return "42ab552ffb4d429a9df7d83aab8d53a1df30269491f2e28c9dd57c7d9187f8e7"
            return sha256(path)

        def recipe(name, *args):
            calls.append(call(name))
            if name == "components/package_openrgb_component.py":
                supplied = args[args.index("--patch") + 1]
                self.assertEqual(supplied, patch_path)
                self.assertEqual(sha256(supplied), locked_digest)
                args[args.index("--output") + 1].write_bytes(original.read_bytes())

        with (
            patch.dict(builder.os.environ, {"ProgramFiles": str(self.root)}),
            patch.multiple(builder, COMPONENT=self.component, run=run, recipe=recipe, sha256=digest),
            patch.object(builder, "checkout", return_value=source),
            patch.object(builder, "fetch", side_effect=lambda _, name, *pin: downloads / name),
            patch.object(verifier, "working_diff", return_value=patch_path.read_bytes()) as exported,
        ):
            result = candidate.load_or_build_component(self.root, vsdevcmd, self.cache, self.key, write_cache=True)
        exported.assert_called_once_with(source)
        self.assertEqual(calls, [
            call("git", "-C", source, "apply", "--check", patch_path),
            call("git", "-C", source, "apply", "--intent-to-add", patch_path),
            call("components/verify_openrgb_component.py"),
            call("components/package_openrgb_component.py"),
            call("components/test_parent_pipe.py"),
        ])
        self.assertEqual(result, self.root / builder.BUNDLE)
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
