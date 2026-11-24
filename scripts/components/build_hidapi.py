"""Rebuild the pinned Windows HIDAPI DLL for the OpenRGB component."""

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import subprocess
from zipfile import ZipFile


def sha256(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def msvc_environment(vsdevcmd):
    setup = f'call "{vsdevcmd}" -no_logo -arch=x64 -host_arch=x64 >NUL && set'
    lines = subprocess.check_output([os.environ["ComSpec"], "/d", "/c", setup], text=True)
    env = {key.upper(): value for key, value in os.environ.items()}
    for line in lines.splitlines():
        key, separator, value = line.partition("=")
        if key and separator:
            env[key.upper()] = value
    for key in ("CL", "_CL_", "LINK", "_LINK_", "CFLAGS", "CXXFLAGS", "LDFLAGS"):
        env.pop(key, None)
    return env


def build(archive, patch, pins, work, vsdevcmd):
    if sha256(archive) != pins["source_archive_sha256"] or sha256(patch) != pins["patch_sha256"]:
        raise ValueError("HIDAPI source archive or patch differs from the dependency lock")
    env = msvc_environment(vsdevcmd)
    toolset = env.get("VCTOOLSVERSION", "").rstrip("\\/")
    sdk = env.get("WINDOWSSDKVERSION", "").rstrip("\\/")
    if (toolset, sdk) != (pins["msvc_toolset"], pins["windows_sdk"]):
        raise ValueError(f"HIDAPI toolchain differs from the dependency lock: MSVC {toolset}, SDK {sdk}")

    source = work / "source"
    source.mkdir(parents=True)
    upstream_root = "hidapi-hotplug-" + pins["source_revision"]
    with ZipFile(archive) as zipped:
        for item in zipped.infolist():
            path = PurePosixPath(item.filename)
            if not path.parts or path.parts[0] != upstream_root or ".." in path.parts or "\\" in item.filename:
                raise ValueError(f"Unexpected HIDAPI archive member: {item.filename}")
            if not item.is_dir():
                target = source.joinpath(*path.parts[1:])
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(zipped.read(item))
    git = ["git", "-C", source]
    subprocess.run([*git, "init", "-q"], check=True)
    apply = [*git, "-c", "core.autocrlf=false", "apply"]
    subprocess.run([*apply, "--check", patch], check=True)
    subprocess.run([*apply, patch], check=True)

    tools = vsdevcmd.parents[2] / "Common7/IDE/CommonExtensions/Microsoft/CMake"
    cmake = tools / "CMake/bin/cmake.exe"
    ninja = tools / "Ninja/ninja.exe"
    output = work / "build"
    subprocess.run([cmake, "-S", source, "-B", output, "-G", "Ninja", f"-DCMAKE_MAKE_PROGRAM={ninja}",
                    "-DCMAKE_BUILD_TYPE=Release", "-DCMAKE_TRY_COMPILE_CONFIGURATION=Release",
                    "-DCMAKE_POLICY_VERSION_MINIMUM=3.5", "-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded",
                    "-DHIDAPI_WITH_TESTS=OFF", "-DHIDAPI_BUILD_PP_DATA_DUMP=OFF", "-DHIDAPI_BUILD_HIDTEST=OFF",
                    "-DCMAKE_SHARED_LINKER_FLAGS=/Brepro"], env=env, check=True)
    subprocess.run([cmake, "--build", output, "--target", "hidapi_winapi", "--verbose"], env=env, check=True)
    dll = output / "src/windows/hidapi-hotplug.dll"
    actual = sha256(dll)
    if actual != pins["sha256"]:
        raise ValueError(f"Rebuilt HIDAPI DLL differs from the dependency lock: {actual}")
    return dll


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("archive", "patch", "dependency-lock", "work", "vsdevcmd", "output"):
        parser.add_argument("--" + name, required=True, type=Path)
    args = parser.parse_args()
    pins = json.loads(args.dependency_lock.read_bytes())["files"]["hidapi-hotplug.dll"]
    dll = build(args.archive.resolve(), args.patch.resolve(), pins, args.work.resolve(), args.vsdevcmd.resolve())
    with dll.open("rb") as source, args.output.open("xb") as output:
        shutil.copyfileobj(source, output)


if __name__ == "__main__":
    main()
