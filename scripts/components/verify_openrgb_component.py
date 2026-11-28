"""Static, fail-closed inventory/import gate; never executes the component."""

import argparse
import hashlib
import json
import re
import stat
import struct
import subprocess
from pathlib import Path


SOURCE_HEAD = "6bf7d9f5348be41dcf6a45dc7ad57f00b0b69b43"
MODULES = ("SmbusPIIX4.bin", "SmbusI801.bin", "SmbusIntelSkylakeIMC.bin",
           "SmbusNCT6793.bin", "LpcIO.bin", "LedsValve.bin")
SOURCE_FILES = {
    "PawnIOLib.dll": "dependencies/PawnIO/PawnIOLib.dll",
    "hidapi-hotplug.dll": "dependencies/hidapi-hotplug-win/x64/hidapi-hotplug.dll",
    "libusb-1.0.dll": "dependencies/libusb-1.0.27/VS2019/MS64/dll/libusb-1.0.dll",
    **{name: "dependencies/PawnIO/modules/" + name for name in MODULES},
}
REQUIRED = {"OpenRGB.exe", *SOURCE_FILES}
# Bundling these files requires a separate redistribution decision. Their presence
# here only permits static inspection; it is not a license approval.
MSVC_RUNTIME = frozenset({"vcruntime140.dll", "vcruntime140_1.dll", "msvcp140.dll",
                          "msvcp140_1.dll", "msvcp140_2.dll", "msvcp140_atomic_wait.dll",
                          "msvcp140_codecvt_ids.dll", "concrt140.dll"})
OS_DLLS = frozenset("""advapi32.dll bcrypt.dll cfgmgr32.dll comctl32.dll comdlg32.dll
crypt32.dll dnsapi.dll dwmapi.dll gdi32.dll hid.dll imm32.dll iphlpapi.dll
kernel32.dll kernelbase.dll mpr.dll msvcrt.dll ncrypt.dll netapi32.dll normaliz.dll
ntdll.dll ole32.dll oleaut32.dll powrprof.dll propsys.dll psapi.dll rpcrt4.dll
secur32.dll setupapi.dll shell32.dll shlwapi.dll user32.dll userenv.dll usp10.dll
ucrtbase.dll version.dll winhttp.dll wininet.dll winmm.dll winspool.drv wintrust.dll
ws2_32.dll wtsapi32.dll""".split())
MAX_PE_BYTES = 128 * 1024 * 1024


class InvalidPE(ValueError):
    pass


def source_built_pawnio(name, pin):
    return (name == "PawnIOLib.dll" and pin.get("build_from_source") is True and "sha256" not in pin
            and re.fullmatch(r"\d+\.\d+\.\d+", str(pin.get("msvc_toolset", ""))) is not None
            and re.fullmatch(r"\d+\.\d+\.\d+\.\d+", str(pin.get("windows_sdk", ""))) is not None)


def pe_imports(data, *, require_pawnio_exports=False):
    """Read normal and RVA-style delay imports of a bounded AMD64 PE32+ image."""
    def unpack(fmt, offset):
        size = struct.calcsize(fmt)
        if offset < 0 or offset + size > len(data):
            raise InvalidPE("truncated PE structure")
        return struct.unpack_from(fmt, data, offset)

    if len(data) > MAX_PE_BYTES or data[:2] != b"MZ":
        raise InvalidPE("invalid DOS header or image size")
    pe, = unpack("<I", 0x3C)
    if data[pe:pe + 4] != b"PE\0\0":
        raise InvalidPE("missing PE signature")
    machine, count, _, _, _, optional_size, _ = unpack("<HHIIIHH", pe + 4)
    optional = pe + 24
    magic, = unpack("<H", optional)
    if machine != 0x8664 or magic != 0x20B or not 1 <= count <= 96 or optional_size < 112:
        raise InvalidPE("unsupported image; AMD64 PE32+ required")
    unpack(f"<{optional_size}s", optional)
    header_size, = unpack("<I", optional + 60)
    directories, = unpack("<I", optional + 108)
    if directories > 16 or 112 + directories * 8 > optional_size:
        raise InvalidPE("invalid data directory count")
    sections = []
    for index in range(count):
        offset = optional + optional_size + index * 40
        unpack("<40s", offset)
        virtual_size, virtual, raw_size, raw = unpack("<IIII", offset + 8)
        if raw_size and (raw < header_size or raw + raw_size > len(data)):
            raise InvalidPE("invalid section raw range")
        sections.append((virtual, max(virtual_size, raw_size), raw, raw_size))
    if header_size < optional + optional_size + count * 40 or header_size > len(data):
        raise InvalidPE("invalid PE header size")

    def file_offset(rva, size):
        matches = []
        if rva < header_size and rva + size <= header_size:
            matches.append(rva)
        for virtual, extent, raw, raw_size in sections:
            if virtual <= rva < virtual + extent:
                relative = rva - virtual
                if relative + size > raw_size:
                    raise InvalidPE("RVA points outside initialized section data")
                matches.append(raw + relative)
        if len(matches) != 1 or size < 0 or matches[0] + size > len(data):
            raise InvalidPE("unmapped or ambiguous RVA")
        return matches[0]

    def ascii_name(rva):
        value = bytearray()
        for index in range(260):
            byte = data[file_offset(rva + index, 1)]
            if not byte:
                break
            value.append(byte)
        else:
            raise InvalidPE("unterminated PE name")
        try:
            return value.decode("ascii")
        except UnicodeDecodeError as error:
            raise InvalidPE("non-ASCII PE name") from error

    def dll_name(rva):
        name = ascii_name(rva).lower()
        if not re.fullmatch(r"[a-z0-9_.-]+\.(?:dll|drv)", name):
            raise InvalidPE("invalid DLL basename")
        return name

    if require_pawnio_exports:
        if not directories:
            raise InvalidPE("PawnIOLib export directory is missing")
        rva, size = unpack("<II", optional + 112)
        if not rva or size < 40:
            raise InvalidPE("invalid PawnIOLib export directory")
        _, functions, count, addresses, names, ordinals = unpack("<6I", file_offset(rva, 40) + 16)
        expected = sorted(f"pawnio_{operation}{suffix}"
                          for operation in ("close", "execute", "execute_async", "load", "open", "version")
                          for suffix in ("", "_nt", "_win32"))
        if functions != count or count != len(expected):
            raise InvalidPE("unexpected PawnIOLib export count")
        name_table = unpack(f"<{count}I", file_offset(names, count * 4))
        ordinal_table = unpack(f"<{count}H", file_offset(ordinals, count * 2))
        address_table = unpack(f"<{count}I", file_offset(addresses, count * 4))
        if ([ascii_name(name) for name in name_table] != expected
                or set(ordinal_table) != set(range(count))):
            raise InvalidPE("unexpected PawnIOLib exports")
        for address in address_table:
            if not address or rva <= address < rva + size:
                raise InvalidPE("PawnIOLib export is missing or forwarded")
            file_offset(address, 1)

    result = {"normal": [], "delay": []}
    for directory, kind, descriptor_size in ((1, "normal", 20), (13, "delay", 32)):
        if directories <= directory:
            continue
        rva, size = unpack("<II", optional + 112 + directory * 8)
        if not rva and not size:
            continue
        if not rva or not descriptor_size <= size <= 4096 * descriptor_size:
            raise InvalidPE("invalid import directory range")
        for relative in range(0, size - descriptor_size + 1, descriptor_size):
            descriptor = file_offset(rva + relative, descriptor_size)
            fields = unpack("<" + "I" * (descriptor_size // 4), descriptor)
            if not any(fields):
                break
            if kind == "delay" and fields[0] != 1:
                raise InvalidPE("unsupported VA-style delay imports")
            result[kind].append(dll_name(fields[3] if kind == "normal" else fields[1]))
        else:
            raise InvalidPE("unterminated import descriptor table")
        result[kind] = sorted(set(result[kind]))
    return result


def regular_file(path):
    info = path.lstat()
    if not stat.S_ISREG(info.st_mode) or getattr(info, "st_file_attributes", 0) & 0x400:
        raise ValueError(f"not a regular non-reparse file: {path}")
    if info.st_size == 0:
        raise ValueError(f"empty file: {path}")
    return info


def fingerprint(path):
    before = regular_file(path)
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    after = regular_file(path)
    if (before.st_size, before.st_mtime_ns, before.st_ino) != (after.st_size, after.st_mtime_ns, after.st_ino):
        raise ValueError(f"file changed during inspection: {path}")
    return {"size": after.st_size, "sha256": digest.hexdigest()}


def git(source, *args):
    return subprocess.run(["git", "-C", str(source), *args], check=True,
                          capture_output=True, text=True, encoding="utf-8", timeout=20).stdout.strip()


def working_diff(source):
    return subprocess.run(["git", "-C", str(source), "diff", "--binary", "--full-index", "HEAD", "--"],
                          check=True, capture_output=True, timeout=20).stdout


def verify(source, release, source_lock=None, dependency_lock=None):
    source, release = Path(source).absolute(), Path(release).absolute()
    report = {"schema_version": 1, "ok": False, "scope": "static_component_candidate",
              "source_root": str(source), "release_directory": str(release),
              "expected_source_head": SOURCE_HEAD, "files": [], "errors": [],
              "missing_msvc_runtime": [],
              "unverified": ["binary built from this source", "license approval",
                             "complete corresponding source", "runtime and hardware behavior",
                             "clean Windows compatibility", "dynamically loaded DLLs"]}
    errors = report["errors"]
    try:
        overrides = {}
        if dependency_lock is not None:
            dependency_data = json.loads(Path(dependency_lock).read_text(encoding="utf-8"))
            if (not isinstance(dependency_data, dict) or type(dependency_data.get("version")) is not int
                    or dependency_data["version"] != 1 or not isinstance(dependency_data.get("files"), dict)):
                raise ValueError("invalid dependency lock")
            overrides = dependency_data["files"]
            if not set(overrides) <= {"PawnIOLib.dll", "hidapi-hotplug.dll", "libusb-1.0.dll"}:
                raise ValueError("dependency overrides are restricted to the three user-mode DLLs")
            for name, item in overrides.items():
                if (not isinstance(item, dict)
                        or not (source_built_pawnio(name, item)
                                or re.fullmatch(r"[0-9a-f]{64}", str(item.get("sha256", ""))))
                        or not re.fullmatch(r"[0-9a-f]{64}", str(item.get("source_archive_sha256", "")))
                        or not re.fullmatch(r"[0-9a-f]{40}", str(item.get("source_revision", "")))):
                    raise ValueError("dependency override needs binary/source hashes and exact revision: " + name)
            report["dependency_overrides"] = overrides
        head = git(source, "rev-parse", "HEAD")
        dirty = git(source, "status", "--porcelain", "--untracked-files=normal")
        report["source_head"], report["source_status"] = head, dirty
        diff = working_diff(source)
        report["working_tree_clean"] = not dirty
        report["tracked_patch"] = {"size": len(diff), "sha256": hashlib.sha256(diff).hexdigest(),
                                   "paths": git(source, "diff", "--name-only", "HEAD", "--").splitlines()}
        report["untracked_paths"] = git(source, "ls-files", "--others", "--exclude-standard").splitlines()
        if head != SOURCE_HEAD:
            errors.append("source HEAD does not match pinned revision")
        if dirty:
            locked = False
            if source_lock is not None:
                lock_path = Path(source_lock)
                lock = json.loads(lock_path.read_text(encoding="utf-8"))
                if not isinstance(lock, dict):
                    raise ValueError("source-lock must be a JSON object")
                patch_name = lock.get("patch", "")
                if not isinstance(patch_name, str) or not re.fullmatch(r"[A-Za-z0-9._-]+\.patch", patch_name):
                    raise ValueError("invalid source-lock patch filename")
                patch_hash = fingerprint(lock_path.parent / patch_name)["sha256"]
                locked = (type(lock.get("version")) is int and lock["version"] == 1
                          and lock.get("revision") == head == SOURCE_HEAD
                          and lock.get("patch_sha256") == patch_hash == report["tracked_patch"]["sha256"]
                          and lock.get("untracked_source_paths") == report["untracked_paths"] == [])
            report["source_patch_locked"] = locked
            if not locked:
                errors.append("source patch provenance gate: dirty tree requires matching source-lock and exported patch with no untracked source")
        project = source / "OpenRGB.pro"
        report["project_file"] = fingerprint(project)
        declared = set(re.findall(r"dependencies/PawnIO/modules/([A-Za-z0-9_.-]+\.bin)", project.read_text(encoding="utf-8")))
        if declared != set(MODULES):
            errors.append("OpenRGB.pro PawnIO module set differs from fixed allowlist")
        directory_info = release.lstat()
        if not stat.S_ISDIR(directory_info.st_mode) or getattr(directory_info, "st_file_attributes", 0) & 0x400:
            raise ValueError("release directory must be a non-reparse directory")
        available = {}
        allowed = {name.lower() for name in REQUIRED} | MSVC_RUNTIME
        for path in sorted(release.iterdir()):
            if path.name.lower() not in allowed:
                errors.append(f"unexpected release entry: {path.name}")
                continue
            if path.name.lower() in available:
                errors.append(f"duplicate case-insensitive filename: {path.name}")
                continue
            try:
                metadata = fingerprint(path)
                entry = {"name": path.name, **metadata}
                available[path.name.lower()] = entry
                report["files"].append(entry)
                if path.suffix.lower() in (".exe", ".dll"):
                    if metadata["size"] > MAX_PE_BYTES:
                        raise InvalidPE("image exceeds size limit")
                    data = path.read_bytes()
                    if hashlib.sha256(data).hexdigest() != metadata["sha256"]:
                        raise ValueError("image changed between hash and import inspection")
                    pawnio = "PawnIOLib.dll"
                    entry["imports"] = pe_imports(data, require_pawnio_exports=(
                        path.name.lower() == pawnio.lower()
                        and source_built_pawnio(pawnio, overrides.get(pawnio, {}))))
                canonical = next((name for name in SOURCE_FILES if name.lower() == path.name.lower()), None)
                if canonical:
                    source_path = source / SOURCE_FILES[canonical]
                    entry["source_path"] = SOURCE_FILES[canonical]
                    entry["source_fingerprint"] = fingerprint(source_path)
                    if canonical in overrides:
                        entry["rebuilt_dependency"] = overrides[canonical]
                        if (not source_built_pawnio(canonical, overrides[canonical])
                                and metadata["sha256"] != overrides[canonical]["sha256"]):
                            errors.append(f"rebuilt dependency hash mismatch: {path.name}")
                    elif metadata != entry["source_fingerprint"]:
                        errors.append(f"source-copy hash/size mismatch: {path.name}")
            except (OSError, ValueError) as error:
                errors.append(f"{path.name}: {error}")
        for name in sorted(REQUIRED):
            if name.lower() not in available:
                errors.append(f"missing required file: {name}")
        for entry in report["files"]:
            for name in sorted(set(sum(entry.get("imports", {}).values(), []))):
                if name.startswith("qt"):
                    errors.append(f"Qt import rejected: {entry['name']} -> {name}")
                elif name in OS_DLLS or re.fullmatch(r"(?:api|ext)-ms-win-[a-z0-9-]+\.dll", name):
                    continue
                elif name in MSVC_RUNTIME and name not in available:
                    report["missing_msvc_runtime"].append(name)
                    errors.append(f"missing MSVC runtime: {entry['name']} -> {name}")
                elif name not in available:
                    errors.append(f"unresolved non-OS import: {entry['name']} -> {name}")
        report["missing_msvc_runtime"] = sorted(set(report["missing_msvc_runtime"]))
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        errors.append(str(error))
    report["ok"] = not errors
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-root", required=True, type=Path)
    parser.add_argument("--release-dir", required=True, type=Path)
    parser.add_argument("--output", type=Path, help="Optional JSON report; JSON is always printed")
    parser.add_argument("--source-lock", type=Path, help="Explicit derivative patch lock; does not imply review/license approval")
    parser.add_argument("--dependency-lock", type=Path, help="Explicit binary/source pins for rebuilt user-mode DLLs")
    args = parser.parse_args()
    report = verify(args.source_root, args.release_dir, args.source_lock, args.dependency_lock)
    encoded = json.dumps(report, ensure_ascii=False, indent=2) + "\n"
    if args.output:
        args.output.write_text(encoded, encoding="utf-8")
    print(encoded, end="")
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
