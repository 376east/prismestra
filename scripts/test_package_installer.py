"""Packaging fixtures exercise archive, prerequisite and isolation boundaries."""

import hashlib
import io
import json
import os
from pathlib import Path
import struct
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import zipfile

import package_installer as installer


def fingerprint(data):
    return hashlib.sha256(data).hexdigest()


def amd64_fixture(linker=(14, 51), subsystem=3):
    pe = bytearray(24 + 240)
    pe[:6] = b"PE\0\0\x64\x86"
    struct.pack_into("<H", pe, 20, 240)
    struct.pack_into("<HBB", pe, 24, 0x20B, *linker)
    struct.pack_into("<H", pe, 24 + 68, subsystem)
    return b"MZ" + bytes(58) + struct.pack("<I", 64) + pe


class InstallerPackaging(unittest.TestCase):
    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.root = Path(folder.name)
        self.app = self.root / "app.exe"
        self.app.write_bytes(amd64_fixture())
        self.namespace = "bf65d1de-875d-40de-8dbc-f6799d222115"

    def component(self, extra=(), incorrect_pin=False, listed=(), linker=(14, 44)):
        source = self.root / "component.zip"
        name = "runtime/OpenRGB.exe"
        image = amd64_fixture(linker)
        pinned = fingerprint(b"wrong") if incorrect_pin else fingerprint(image)
        manifest = {
            "version": 1, "scope": "packaging_only", "engine_revision": "b" * 40,
            "engine_sha256": pinned,
            "files": [{"path": name, "sha256": pinned, "size": len(image)}],
        }
        for item in listed:
            manifest["files"].append({"path": item, "sha256": fingerprint(b"fixture"), "size": 7})
        with zipfile.ZipFile(source, "w") as archive:
            for item in listed:
                archive.writestr(item, b"fixture")
            archive.writestr(name, image)
            archive.writestr("component-manifest.json", json.dumps(manifest))
            for path in extra:
                archive.writestr(path, b"fixture")
        return source

    def test_nested_build_recipe_is_preserved_but_nested_runtime_is_rejected(self):
        recipe = "build/pawniolib/CMakeLists.txt"
        archive = self.component(listed=(recipe,))
        destination = self.root / "component"
        installer.extract_component(archive, destination, installer.sha256(archive))
        self.assertEqual((destination / recipe).read_bytes(), b"fixture")
        archive = self.component(listed=("runtime/unknown/child.dll",))
        with self.assertRaisesRegex(ValueError, "Unexpected component directory"):
            installer.extract_component(archive, self.root / "bad", installer.sha256(archive))

    def test_component_rejects_traversal_and_windows_reserved_names(self):
        for name in ("../escape.exe", "runtime/../escape.exe", "runtime/COM1.exe", "runtime/x.dll:stream"):
            with self.subTest(name=name):
                archive = self.component(extra=(name,))
                destination = self.root / "staged"
                with self.assertRaisesRegex(ValueError, "Unsafe archive path"):
                    installer.extract_component(archive, destination, installer.sha256(archive))
                self.assertFalse(destination.exists())
        self.assertFalse((self.root / "escape.exe").exists())

    def test_component_rejects_case_insensitive_duplicate(self):
        archive = self.component(extra=("runtime/openrgb.exe",))
        with self.assertRaisesRegex(ValueError, "repeated component"):
            installer.extract_component(archive, self.root / "stage", installer.sha256(archive))

    def test_component_rejects_unlisted_file(self):
        archive = self.component(extra=("runtime/unlisted.dll",))
        with self.assertRaisesRegex(ValueError, "differ from the manifest"):
            installer.extract_component(archive, self.root / "stage", installer.sha256(archive))

    def test_component_validates_inner_file_pins_even_with_valid_outer_pin(self):
        archive = self.component(incorrect_pin=True)
        with self.assertRaisesRegex(ValueError, "unpinned file: OpenRGB.exe"):
            installer.extract_component(archive, self.root / "stage", installer.sha256(archive))

    def prerequisite_lock(self):
        lock = json.loads((installer.ROOT / "installer/prerequisites.lock.json").read_text())
        lock["packages"].pop("pawnio")
        return lock

    def test_runtime_floor_rejects_malformed_and_newer_than_bundle(self):
        lock = self.prerequisite_lock()
        path = self.root / "prerequisites.json"
        installer.write_json(path, lock)
        _, floor = installer.read_prerequisite_lock(path)
        self.assertEqual(floor, (14, 51, 36247, 0))
        for value in (None, "14.51", "14.51.65536.0", "14.53.0.0"):
            with self.subTest(floor=value):
                lock["minimum_vc_runtime"] = value
                installer.write_json(path, lock)
                with self.assertRaisesRegex(ValueError, "minimum_vc_runtime"):
                    installer.read_prerequisite_lock(path)

    def test_linker_above_runtime_floor_is_rejected_before_copy(self):
        lock = self.root / "prerequisites.json"
        installer.write_json(lock, self.prerequisite_lock())
        _, floor = installer.read_prerequisite_lock(lock)
        archive = self.component(linker=(14, 52))
        backend = self.root / "staged-backend"
        with self.assertRaisesRegex(ValueError, r"OpenRGB.exe linker 14\.52"):
            installer.extract_component(archive, backend, installer.sha256(archive), floor)
        self.assertFalse(backend.exists())

        self.app.write_bytes(amd64_fixture((14, 52)))
        args = self.arguments(
            test_namespace=None, prerequisite_lock=lock, prerequisite_dir=self.root, release_dir=self.root,
        )
        with self.assertRaisesRegex(ValueError, r"Prismestra.exe linker 14\.52"):
            installer.package(args)
        self.assertFalse(args.output.exists())

    def test_prerequisite_rejects_bootstrapper_metadata_before_authenticode(self):
        lock = self.prerequisite_lock()
        lock["packages"]["vc_redist_x64"]["file"] = "download-vc.exe"
        path = self.root / "prerequisites.json"
        installer.write_json(path, lock)
        with patch.object(installer, "authenticode") as verify:
            with self.assertRaisesRegex(ValueError, "Invalid offline prerequisite pin"):
                packages, _ = installer.read_prerequisite_lock(path)
                installer.stage_prerequisites(packages, self.root, self.root / "staged")
        verify.assert_not_called()

    def test_unsigned_prerequisite_is_delegated_to_authenticode_and_rejected(self):
        lock = self.prerequisite_lock()
        item = lock["packages"]["vc_redist_x64"]
        payload = self.root / item["file"]
        with payload.open("wb") as stream:
            stream.truncate(11 << 20)
        item["sha256"] = installer.sha256(payload)
        item["size"] = payload.stat().st_size
        path = self.root / "prerequisites.json"
        installer.write_json(path, lock)
        # Simulate WinVerifyTrust's rejection. The asset is never used as a command.
        with patch.object(installer.sys, "platform", "win32"), patch.dict(
            installer.os.environ, {"SystemRoot": str(self.root)}
        ), patch.object(installer.subprocess, "run", return_value=SimpleNamespace(
            returncode=1, stderr="not signed", stdout=""
        )) as execute:
            with self.assertRaisesRegex(ValueError, "Authenticode validation failed"):
                packages, _ = installer.read_prerequisite_lock(path)
                installer.stage_prerequisites(packages, self.root, self.root / "staged")
        self.assertEqual(Path(execute.call_args.args[0][0]).name, "powershell.exe")
        self.assertNotIn(str(payload), execute.call_args.args[0])

    def arguments(self, **overrides):
        component = self.component()
        defaults = dict(
            version="0.1.0.5", expected_commit="a" * 40,
            app_exe=self.app, app_sha256=installer.sha256(self.app),
            component_zip=component, component_sha256=installer.sha256(component),
            output=self.root / "first", test_namespace=self.namespace,
            prerequisite_lock=None, prerequisite_dir=None, release_dir=None,
            stage_only=True, iscc=None,
        )
        return SimpleNamespace(**(defaults | overrides))

    def test_namespace_and_version_reject_traversal_and_non_numeric_versions(self):
        with self.assertRaisesRegex(ValueError, "lowercase GUID"):
            installer.package(self.arguments(test_namespace="../profile"))
        with self.assertRaisesRegex(ValueError, "differ from the production"):
            installer.package(self.arguments(test_namespace=installer.APP_ID))
        with self.assertRaisesRegex(ValueError, "numeric installer version"):
            installer.package(self.arguments(version="0.1.0-preview.5"))
        with self.assertRaisesRegex(ValueError, "cannot contain prerequisites"):
            installer.package(self.arguments(prerequisite_lock=self.root / "fake.lock"))

    @unittest.skipUnless(os.name == "nt", "Requires inbox Windows PowerShell")
    def test_generated_standard_user_worker_has_unicode_bom_and_parses_in_powershell_5(self):
        script = r'''
$ErrorActionPreference = 'Stop'
$tokens = $null; $errors = $null
$text = [IO.File]::ReadAllText($env:PRISMESTRA_WORKER_SOURCE, [Text.UTF8Encoding]::new($false, $true))
$ast = [Management.Automation.Language.Parser]::ParseInput($text, [ref]$tokens, [ref]$errors)
if ($errors) { throw ($errors | Out-String) }
$assignment = $ast.Find({param($a) $a -is [Management.Automation.Language.AssignmentStatementAst] -and $a.Left.Extent.Text -ceq '$worker'}, $true)
$literal = $assignment.Find({param($a) $a -is [Management.Automation.Language.StringConstantExpressionAst] -and $a.StringConstantType.ToString() -ceq 'SingleQuotedHereString'}, $true)
$writer = $ast.Find({param($a) $a -is [Management.Automation.Language.InvokeMemberExpressionAst] -and $a.Member.Value -ceq 'WriteAllText' -and $a.Arguments[0].Extent.Text -ceq '$workerFile'}, $true)
if (-not $literal -or -not $writer) { throw 'Generated worker source or writer missing' }
$worker = $literal.Value
$workerFile = $env:PRISMESTRA_WORKER_FIXTURE
# Execute only the real file-generation expression, never the test's account/installer actions.
& ([scriptblock]::Create($writer.Extent.Text))
$bytes = [IO.File]::ReadAllBytes($workerFile)
if ($bytes.Length -lt 3 -or $bytes[0] -ne 239 -or $bytes[1] -ne 187 -or $bytes[2] -ne 191) {
    throw 'Windows PowerShell worker lacks its UTF-8 BOM'
}
[void][Management.Automation.Language.Parser]::ParseFile($workerFile, [ref]$tokens, [ref]$errors)
if ($errors) { throw ($errors | Out-String) }
$assignment = $ast.Find({param($a) $a -is [Management.Automation.Language.AssignmentStatementAst] -and $a.Left.Extent.Text -ceq '$wrapper'}, $true)
$literal = $assignment.Find({param($a) $a -is [Management.Automation.Language.StringConstantExpressionAst] -and $a.StringConstantType.ToString() -ceq 'SingleQuotedHereString'}, $true)
$wrapper = $literal.Value
$wrapperFile = Join-Path (Split-Path $workerFile) 'invoke-worker.ps1'
$writer = $ast.Find({param($a) $a -is [Management.Automation.Language.InvokeMemberExpressionAst] -and $a.Member.Value -ceq 'WriteAllText' -and $a.Arguments[0].Extent.Text -ceq '$wrapperFile'}, $true)
& ([scriptblock]::Create($writer.Extent.Text))
[void][Management.Automation.Language.Parser]::ParseFile($wrapperFile, [ref]$tokens, [ref]$errors)
if ($errors) { throw ($errors | Out-String) }
$id = '01234567-89ab-cdef-0123-456789abcdef'
$install = 'C:\Program Files\Prismestra Lifecycle Test\' + $id
$sid = 'S-1-5-21-1234567890-1234567890-1234567890-1001'
$powershell = Join-Path $PSHOME 'powershell.exe'
$arguments = $ast.Find({param($a) $a -is [Management.Automation.Language.AssignmentStatementAst] -and $a.Left.Extent.Text -ceq '$workerArguments'}, $true)
$workerArguments = & ([scriptblock]::Create($arguments.Right.Extent.Text))
if (($powershell.Length + $workerArguments.Length + 4) -ge 1024) { throw 'Fixture exceeds CreateProcessWithLogonW command-line limit' }
# Exercise the real wrapper with harmless scripts, never the account/installer worker.
[IO.File]::WriteAllText($workerFile, 'Write-Output "fixture success"; exit 0', [Text.UTF8Encoding]::new($true))
& $powershell -NoProfile -NonInteractive -File $wrapperFile $install $id $sid
if ($LASTEXITCODE -ne 0) { throw 'Wrapper failed to return successful status' }
$log = Join-Path (Split-Path $workerFile) 'worker-output.log'
if ([IO.File]::ReadAllText($log) -notmatch 'fixture success') { throw 'Wrapper lost output' }
[IO.File]::WriteAllText($workerFile, 'throw "fixture failure"', [Text.UTF8Encoding]::new($true))
& $powershell -NoProfile -NonInteractive -File $wrapperFile $install $id $sid
if ($LASTEXITCODE -ne 1 -or [IO.File]::ReadAllText($log) -notmatch 'fixture failure') { throw 'Wrapper lost failure status or diagnostic' }
exit 0
'''
        import base64
        powershell = Path(os.environ["SystemRoot"]) / "System32/WindowsPowerShell/v1.0/powershell.exe"
        result = subprocess.run(
            [str(powershell), "-NoProfile", "-NonInteractive", "-EncodedCommand",
             base64.b64encode(script.encode("utf-16-le")).decode()],
            env=os.environ | {
                "PRISMESTRA_WORKER_SOURCE": str(installer.ROOT / "installer/test_standard_user.ps1"),
                "PRISMESTRA_WORKER_FIXTURE": str(self.root / "worker.ps1"),
            }, capture_output=True, text=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_isolated_stage_is_deterministic_and_upgrade_preserves_namespace(self):
        first = self.arguments()
        same = SimpleNamespace(**(vars(first) | {"output": self.root / "same"}))
        upgrade = SimpleNamespace(**(vars(first) | {
            "version": "0.1.0.6", "output": self.root / "upgrade",
        }))
        self.assertEqual(installer.package(first), installer.package(same))
        installer.package(upgrade)
        manifests = [
            json.loads((args.output / "stage/payload/install-manifest.json").read_bytes())
            for args in (first, upgrade)
        ]
        self.assertEqual([m["installer_version"] for m in manifests], ["0.1.0.5", "0.1.0.6"])
        self.assertTrue(all(m["data_namespace"] == self.namespace and not m["prerequisites"] for m in manifests))
        self.assertTrue(all(m["backend"]["relative_executable"].endswith("/runtime/OpenRGB.exe") for m in manifests))

    def test_setup_stages_only_native_companions_with_expected_imports(self):
        policy = amd64_fixture(subsystem=2) + b"policy"
        legacy = amd64_fixture(subsystem=2) + b"legacy"

        def fake_build(command, **_kwargs):
            output = Path(command[command.index("--target-dir") + 1]) / "x86_64-pc-windows-msvc/release"
            output.mkdir(parents=True)
            (output / "prismestra-setup-policy.exe").write_bytes(policy)
            (output / "prismestra-legacy-startup.exe").write_bytes(legacy)

        with patch.object(installer.subprocess, "run", side_effect=fake_build) as build, patch.object(
            installer, "pe_imports", side_effect=[
                {"normal": ["kernel32.dll"], "delay": []},
                {"normal": ["kernel32.dll", "user32.dll"], "delay": []},
            ]
        ):
            hashes = installer.stage_setup_tools(self.root)
        self.assertIn("--bins", build.call_args.args[0])
        self.assertEqual(build.call_args.kwargs["env"]["CARGO_ENCODED_RUSTFLAGS"], "-C\x1ftarget-feature=+crt-static")
        self.assertEqual(hashes, {
            installer.POLICY_TOOL: fingerprint(policy),
            installer.LEGACY_STARTUP_TOOL: fingerprint(legacy),
        })

    def test_native_setup_companions_cannot_use_the_console_subsystem(self):
        with self.assertRaisesRegex(ValueError, "Windows GUI executable"):
            installer.check_amd64_executable(io.BytesIO(amd64_fixture()), "helper.exe", require_gui=True)

    def test_setup_and_uninstall_have_no_shell_entry_and_keep_legacy_order(self):
        source = (installer.ROOT / "installer/Prismestra.iss").read_text(encoding="utf-8")
        guard = (installer.ROOT / "installer/guard-native.iss").read_text(encoding="utf-8")
        for shell in ("powershell.exe", "pwsh.exe", "cmd.exe", "sh.exe", ".ps1"):
            self.assertNotIn(shell, (source + guard).lower())
        retirement = source.split("procedure RetireLegacyStartup;", 1)[1]
        self.assertLess(retirement.index("LegacyStartup('Prepare'"), retirement.index("--setup-prepare-installed"))
        self.assertLess(retirement.index("--setup-prepare-installed"), retirement.index("LegacyStartup('Disable'"))
        self.assertIn("RunManagedPolicy('--uninstall'", source)

    def test_setup_notifies_only_interactive_verified_legacy_deferrals(self):
        source = (installer.ROOT / "installer/Prismestra.iss").read_text(encoding="utf-8")
        legacy = source.split("function LegacyStartup(", 1)[1].split("#endif", 1)[0]
        self.assertIn("if Action = 'Disable' then Args := Args + ' --setup-outcome';", legacy)
        self.assertIn("GetSHA256OfFile(Tool) = '{#LegacyStartupToolSha256}'", legacy)
        self.assertRegex(
            legacy,
            r"Code := LegacyStartup\('Disable', SID\);\s*(?:\{[^}]*\}\s*)?"
            r"if \(Code = 20\) or \(Code = 21\) then begin\s*Log\([^\n]*\);"
            r"\s*if not WizardSilent then\s*SuppressibleMsgBox\(",
        )
        self.assertEqual(source.count("RetireLegacyStartup"), 2)

    @unittest.skipUnless(os.name == "nt", "Requires the x64 Windows Rust toolchain")
    def test_builds_webview_probe_without_a_redistributable_and_checks_versions(self):
        probe_hash = installer.stage_webview_probe(self.root)
        self.assertEqual(probe_hash, installer.sha256(self.root / installer.WEBVIEW_PROBE))
        test_image = self.root / "probe-tests.exe"
        subprocess.run([
            "rustc", "--edition", "2024", "--target", "x86_64-pc-windows-msvc",
            "-C", "target-feature=+crt-static", "--test", "--crate-name", "prismestra_setup_probe_tests",
            "-o", str(test_image), str(installer.ROOT / "installer/webview-probe.rs"),
        ], check=True)
        subprocess.run([str(test_image)], check=True)


if __name__ == "__main__":
    unittest.main()
