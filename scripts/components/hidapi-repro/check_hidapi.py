"""Build only pinned Windows HIDAPI twice and print a hash receipt."""

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import struct
import subprocess
import tempfile
import time
from zipfile import ZipFile


REVISION = "238df449ecc0a56a70a3c8f4ebf46bb46857af27"
ARCHIVE_SHA256 = "d4197d1756dbb0f6d7ff167d1e92ec5b44d9d9ce6b87cb8ff3603272edcb1817"
PATCH_SHA256 = "a5f9b93d20d1cfd4e37dd544deebc9bdd31f41b162ded3bd63c1dbfcd32765c2"
LOCAL_DLL_SHA256 = "d77293458844bba20526b8f0cecf1691e73ec1d613a8083607f2406296e67ede"


def sha256(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def recreate_source(archive, patch, source):
    source.mkdir()
    with ZipFile(archive) as zipped:
        for item in zipped.infolist():
            path = PurePosixPath(item.filename)
            if path.parts[0] != f"hidapi-hotplug-{REVISION}" or ".." in path.parts:
                raise ValueError(f"Unexpected pinned archive entry: {item.filename}")
            if not item.is_dir():
                target = source.joinpath(*path.parts[1:])
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(zipped.read(item))
    git = ["git", "-C", source]
    subprocess.run([*git, "init", "-q"], check=True)
    apply = [*git, "-c", "core.autocrlf=false", "apply"]
    subprocess.run([*apply, "--check", patch], check=True)
    subprocess.run([*apply, patch], check=True)


def linker_version(path):
    image = path.read_bytes()
    offset, = struct.unpack_from("<I", image, 0x3c)
    if image[:2] != b"MZ" or image[offset:offset + 4] != b"PE\0\0":
        raise ValueError("Expected a PE DLL")
    machine, = struct.unpack_from("<H", image, offset + 4)
    if machine != 0x8664:
        raise ValueError("Expected an AMD64 DLL")
    return list(image[offset + 26:offset + 28])


def tools_from_cache(build, cmake):
    names = {
        "CMAKE_C_COMPILER": "cl.exe", "CMAKE_LINKER": "link.exe",
        "CMAKE_RC_COMPILER": "rc.exe", "CMAKE_MT": "mt.exe", "CMAKE_MAKE_PROGRAM": "ninja.exe",
    }
    found = {"cmake.exe": sha256(cmake)}
    for line in (build / "CMakeCache.txt").read_text(encoding="utf-8").splitlines():
        key, separator, path = line.partition(":FILEPATH=")
        if separator and key in names:
            found[names[key]] = sha256(Path(path))
    if set(found) != {*names.values(), "cmake.exe"}:
        raise ValueError("CMake did not report all expected MSVC build tools")
    return found


def exports(path):
    dump = subprocess.check_output(["dumpbin.exe", "/nologo", "/exports", path]).decode(errors="replace")
    return {match[2]: int(match[1]) for match in
            re.finditer(r"^\s+(\d+)\s+[A-F0-9]+\s+[A-F0-9]+\s+(\w+)\b", dump, re.M)}


def build(archive, patch, cmake, ninja, parent):
    source = parent / "source"
    output = parent / "build"
    recreate_source(archive, patch, source)
    env = os.environ.copy()
    for name in ("CL", "_CL_", "LINK", "_LINK_", "CFLAGS", "CXXFLAGS", "LDFLAGS"):
        env.pop(name, None)
    subprocess.run([cmake, "-S", source, "-B", output, "-G", "Ninja",
                    f"-DCMAKE_MAKE_PROGRAM={ninja}", "-DCMAKE_BUILD_TYPE=Release",
                    "-DCMAKE_TRY_COMPILE_CONFIGURATION=Release", "-DCMAKE_POLICY_VERSION_MINIMUM=3.5",
                    "-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded", "-DHIDAPI_WITH_TESTS=OFF",
                    "-DHIDAPI_BUILD_PP_DATA_DUMP=OFF", "-DHIDAPI_BUILD_HIDTEST=OFF",
                    "-DCMAKE_SHARED_LINKER_FLAGS=/Brepro"], env=env, check=True)
    subprocess.run([cmake, "--build", output, "--target", "hidapi_winapi", "--verbose"], env=env, check=True)
    dll = output / "src/windows/hidapi-hotplug.dll"
    return {"sha256": sha256(dll), "pe_linker": linker_version(dll),
            "tool_sha256": tools_from_cache(output, cmake), "exports": exports(dll)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", required=True, type=Path)
    parser.add_argument("--patch", required=True, type=Path)
    parser.add_argument("--scratch", required=True, type=Path)
    parser.add_argument("--cmake", default=shutil.which("cmake"), type=Path)
    parser.add_argument("--ninja", default=shutil.which("ninja"), type=Path)
    parser.add_argument("--require-local-sha", action="store_true")
    args = parser.parse_args()
    if os.name != "nt" or not os.environ.get("VCToolsVersion") or not os.environ.get("WindowsSDKVersion"):
        parser.error("Use an initialized Windows x64 MSVC developer environment")
    if not args.cmake or not args.ninja:
        parser.error("CMake and Ninja are required")
    archive = args.archive.resolve(strict=True)
    patch = args.patch.resolve(strict=True)
    if sha256(archive) != ARCHIVE_SHA256 or sha256(patch) != PATCH_SHA256:
        parser.error("HIDAPI archive or patch SHA-256 differs from the probe pins")
    fixture = json.loads(Path(__file__).with_name("preview-abi.json").read_bytes())
    dependency_lock = Path(__file__).resolve().parents[3] / "components/openrgb/dependency-lock.json"
    baseline = json.loads(dependency_lock.read_bytes())["files"]["hidapi-hotplug.dll"]["sha256"]
    if fixture["dll_sha256"] != baseline or len(fixture["exports"]) != 30:
        parser.error("ABI fixture must describe the DLL pinned at the experiment base")
    timings = []
    with tempfile.TemporaryDirectory(dir=args.scratch, prefix="hid-") as scratch:
        results = []
        for name in ("a", "b"):
            directory = Path(scratch) / name
            directory.mkdir()
            started = time.monotonic()
            results.append(build(archive, patch, args.cmake, args.ninja, directory))
            timings.append(round(time.monotonic() - started, 3))
    abi_match = all(result["exports"] == fixture["exports"] for result in results)
    receipt = {"source_revision": REVISION, "archive_sha256": ARCHIVE_SHA256, "patch_sha256": PATCH_SHA256,
               "image": os.environ.get("ImageOS"), "image_version": os.environ.get("ImageVersion"),
               "msvc_toolset": os.environ["VCToolsVersion"],
               "windows_sdk": os.environ["WindowsSDKVersion"].rstrip("\\/"),
               "cmake_version": subprocess.check_output([args.cmake, "--version"], text=True).splitlines()[0],
               "ninja_version": subprocess.check_output([args.ninja, "--version"], text=True).strip(),
               "expected_local_sha256": LOCAL_DLL_SHA256, "builds": results,
               "elapsed_seconds": timings, "preview_abi_baseline_sha256": baseline,
               "preview_export_names_and_ordinals_match": abi_match,
               "same_runner_equal": results[0] == results[1],
               "matches_local_sha256": all(result["sha256"] == LOCAL_DLL_SHA256 for result in results)}
    print(json.dumps(receipt, indent=2), flush=True)
    return int(not abi_match or not receipt["same_runner_equal"] or
               (args.require_local_sha and not receipt["matches_local_sha256"]))


if __name__ == "__main__":
    raise SystemExit(main())
