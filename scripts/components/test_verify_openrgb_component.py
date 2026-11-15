"""Synthetic static PE fixtures only; no binaries are executed."""

import hashlib
import importlib.util
import json
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location("component_verifier", Path(__file__).with_name("verify_openrgb_component.py"))
verifier = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(verifier)


def image(normal=(), delay=()):
    data = bytearray(2560)
    data[:2] = b"MZ"
    struct.pack_into("<I", data, 0x3C, 0x80)
    data[0x80:0x84] = b"PE\0\0"
    struct.pack_into("<HHIIIHH", data, 0x84, 0x8664, 1, 0, 0, 0, 240, 0)
    optional = 0x98
    struct.pack_into("<H", data, optional, 0x20B)
    struct.pack_into("<I", data, optional + 60, 512)
    struct.pack_into("<I", data, optional + 108, 16)
    section = optional + 240
    struct.pack_into("<IIII", data, section + 8, 2048, 4096, 2048, 512)
    for names, directory, base, descriptor_size in ((normal, 1, 512, 20), (delay, 13, 1280, 32)):
        if not names:
            continue
        rva = 4096 + base - 512
        struct.pack_into("<II", data, optional + 112 + directory * 8, rva, descriptor_size * (len(names) + 1))
        name_offset = base + 384
        for index, name in enumerate(names):
            encoded = name.encode("ascii") + b"\0"
            name_rva = 4096 + name_offset - 512
            if directory == 1:
                struct.pack_into("<IIIII", data, base + index * descriptor_size, 0, 0, 0, name_rva, 1)
            else:
                struct.pack_into("<IIIIIIII", data, base + index * descriptor_size, 1, name_rva, 0, 0, 0, 0, 0, 0)
            data[name_offset:name_offset + len(encoded)] = encoded
            name_offset += len(encoded)
    return bytes(data)


class PEChecks(unittest.TestCase):
    def test_normal_and_delay_imports_are_case_normalized(self):
        self.assertEqual(verifier.pe_imports(image(["KERNEL32.dll"], ["Qt6Core.dll"])),
                         {"normal": ["kernel32.dll"], "delay": ["qt6core.dll"]})

    def test_malformed_pe_is_rejected(self):
        fixtures = [b"", b"MZ", image()[:300]]
        wrong_machine = bytearray(image())
        struct.pack_into("<H", wrong_machine, 0x84, 0x14C)
        fixtures.append(wrong_machine)
        unmapped = bytearray(image(["kernel32.dll"]))
        struct.pack_into("<I", unmapped, 512 + 12, 0xFFFFFF00)
        fixtures.append(unmapped)
        unterminated = bytearray(image(["kernel32.dll"]))
        struct.pack_into("<I", unterminated, 0x98 + 112 + 8 + 4, 20)
        fixtures.append(unterminated)
        va_delay = bytearray(image(delay=["kernel32.dll"]))
        struct.pack_into("<I", va_delay, 1280, 0)
        fixtures.append(va_delay)
        for data in fixtures:
            with self.subTest(size=len(data)), self.assertRaises(verifier.InvalidPE):
                verifier.pe_imports(data)


class CandidateChecks(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="openrgb-static-")
        self.addCleanup(self.temporary.cleanup)
        self.base = Path(self.temporary.name)
        self.source = self.base / "source"
        self.release = self.base / "release"
        self.source.mkdir()
        self.release.mkdir()
        (self.source / "OpenRGB.pro").write_text("\n".join(verifier.SOURCE_FILES.values()), encoding="utf-8")
        for name, relative in verifier.SOURCE_FILES.items():
            data = image() if name.endswith(".dll") else b"module " + name.encode()
            path = self.source / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
            (self.release / name).write_bytes(data)
        (self.release / "OpenRGB.exe").write_bytes(image(["KERNEL32.dll", "PawnIOLib.dll", "WS2_32.dll"]))
        self.head, self.status = verifier.SOURCE_HEAD, ""
        self.untracked = ""

        def fake_git(source, *args):
            if args == ("rev-parse", "HEAD"):
                return self.head
            if args[0] == "status":
                return self.status
            if args[0] == "ls-files":
                return self.untracked
            return "patched.cpp" if self.status else ""

        self.git = patch.object(verifier, "git", side_effect=fake_git)
        self.diff = patch.object(verifier, "working_diff", return_value=b"reviewable patch\n")
        self.git.start()
        self.diff.start()
        self.addCleanup(self.git.stop)
        self.addCleanup(self.diff.stop)

    def verify(self):
        return verifier.verify(self.source, self.release)

    def test_complete_candidate_records_exact_hashes_and_limits_scope(self):
        report = self.verify()
        self.assertTrue(report["ok"], report["errors"])
        exe = next(item for item in report["files"] if item["name"] == "OpenRGB.exe")
        data = (self.release / "OpenRGB.exe").read_bytes()
        self.assertEqual(exe["size"], len(data))
        self.assertEqual(exe["sha256"], hashlib.sha256(data).hexdigest())
        self.assertIn("license approval", report["unverified"])

    def test_missing_required_and_unknown_files_fail_closed(self):
        (self.release / "LedsValve.bin").unlink()
        (self.release / "surprise.dll").write_bytes(image())
        report = self.verify()
        self.assertFalse(report["ok"])
        self.assertIn("missing required file: LedsValve.bin", report["errors"])
        self.assertIn("unexpected release entry: surprise.dll", report["errors"])

    def test_source_copy_tampering_is_detected(self):
        (self.release / "LpcIO.bin").write_bytes(b"tampered")
        self.assertTrue(any("source-copy hash/size mismatch" in error for error in self.verify()["errors"]))

    def test_rebuilt_dll_requires_explicit_binary_and_source_pins(self):
        data = image(["ntdll.dll"])
        (self.release / "PawnIOLib.dll").write_bytes(data)
        self.assertFalse(self.verify()["ok"])
        lock = self.base / "dependencies.json"
        item = {"sha256": hashlib.sha256(data).hexdigest(), "source_archive_sha256": "a" * 64,
                "source_revision": "b" * 40}
        lock.write_text(json.dumps({"version": 1, "files": {"PawnIOLib.dll": item}}), encoding="utf-8")
        report = verifier.verify(self.source, self.release, dependency_lock=lock)
        self.assertTrue(report["ok"], report["errors"])
        item["sha256"] = "c" * 64
        lock.write_text(json.dumps({"version": 1, "files": {"PawnIOLib.dll": item}}), encoding="utf-8")
        self.assertFalse(verifier.verify(self.source, self.release, dependency_lock=lock)["ok"])

    def test_dependency_override_cannot_bypass_pawnio_module_copy_gate(self):
        lock = self.base / "dependencies.json"
        lock.write_text(json.dumps({"version": 1, "files": {"LpcIO.bin": {}}}), encoding="utf-8")
        report = verifier.verify(self.source, self.release, dependency_lock=lock)
        self.assertFalse(report["ok"])
        self.assertTrue(any("restricted to the three" in error for error in report["errors"]))

    def test_qt_normal_and_delay_imports_are_rejected(self):
        for normal, delay in ((["Qt6Core.dll"], []), ([], ["Qt5Network.dll"])):
            with self.subTest(normal=normal, delay=delay):
                (self.release / "OpenRGB.exe").write_bytes(image(normal, delay))
                report = self.verify()
                self.assertFalse(report["ok"])
                self.assertTrue(any("Qt import rejected" in error for error in report["errors"]))

    def test_missing_runtime_is_explicit_and_bundled_runtime_is_inspected(self):
        (self.release / "OpenRGB.exe").write_bytes(image(["VCRUNTIME140.dll"]))
        report = self.verify()
        self.assertFalse(report["ok"])
        self.assertEqual(report["missing_msvc_runtime"], ["vcruntime140.dll"])
        (self.release / "vcruntime140.dll").write_bytes(image(["kernel32.dll"]))
        report = self.verify()
        self.assertTrue(report["ok"], report["errors"])

    def test_unknown_dependency_is_not_assumed_to_be_an_os_dll(self):
        (self.release / "OpenRGB.exe").write_bytes(image(["vendor.dll"]))
        self.assertTrue(any("unresolved non-OS import" in error for error in self.verify()["errors"]))

    def test_invalid_required_pe_is_a_gate(self):
        (self.release / "OpenRGB.exe").write_bytes(b"bad PE")
        self.assertFalse(self.verify()["ok"])

    def test_module_allowlist_drift_is_rejected(self):
        (self.source / "OpenRGB.pro").write_text("dependencies/PawnIO/modules/new.bin", encoding="utf-8")
        self.assertFalse(self.verify()["ok"])

    def test_pinned_head_and_dirty_patch_provenance_are_separate_gates(self):
        self.head = "0" * 40
        self.status = " M patched.cpp\n?? new.cpp"
        self.untracked = "new.cpp"
        report = self.verify()
        self.assertFalse(report["ok"])
        self.assertFalse(report["working_tree_clean"])
        self.assertEqual(report["tracked_patch"]["sha256"], hashlib.sha256(b"reviewable patch\n").hexdigest())
        self.assertEqual(report["tracked_patch"]["paths"], ["patched.cpp"])
        self.assertEqual(report["untracked_paths"], ["new.cpp"])
        self.assertTrue(any("pinned revision" in error for error in report["errors"]))
        self.assertTrue(any("patch provenance gate" in error for error in report["errors"]))

    def test_derivative_requires_exact_exported_patch_and_no_untracked_source(self):
        self.status = " M patched.cpp"
        patch_data = b"reviewable patch\n"
        (self.base / "candidate.patch").write_bytes(patch_data)
        lock = self.base / "source-lock.json"
        lock.write_text(json.dumps({"version": 1, "revision": self.head,
                                    "patch": "candidate.patch",
                                    "patch_sha256": hashlib.sha256(patch_data).hexdigest(),
                                    "untracked_source_paths": []}), encoding="utf-8")
        report = verifier.verify(self.source, self.release, lock)
        self.assertTrue(report["ok"], report["errors"])
        self.assertTrue(report["source_patch_locked"])
        (self.base / "candidate.patch").write_bytes(b"different exported patch")
        self.assertFalse(verifier.verify(self.source, self.release, lock)["ok"])
        (self.base / "candidate.patch").write_bytes(patch_data)
        self.untracked = "unreviewed.cpp"
        self.assertFalse(verifier.verify(self.source, self.release, lock)["ok"])

    def test_source_lock_cannot_escape_its_directory(self):
        self.status = " M patched.cpp"
        lock = self.base / "source-lock.json"
        lock.write_text(json.dumps({"patch": "../candidate.patch"}), encoding="utf-8")
        report = verifier.verify(self.source, self.release, lock)
        self.assertFalse(report["ok"])
        self.assertIn("invalid source-lock patch filename", report["errors"])

    def test_directory_instead_of_required_file_is_rejected(self):
        path = self.release / "LpcIO.bin"
        path.unlink()
        path.mkdir()
        self.assertFalse(self.verify()["ok"])

    def test_symlink_is_rejected_without_reading_target(self):
        path = self.release / "LpcIO.bin"
        path.unlink()
        try:
            path.symlink_to(self.source / verifier.SOURCE_FILES[path.name])
        except OSError:
            self.skipTest("Creating symlinks is unavailable to this Windows user")
        self.assertFalse(self.verify()["ok"])


if __name__ == "__main__":
    unittest.main()
