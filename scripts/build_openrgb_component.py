"""Build and smoke-check the pinned OpenRGB component for Windows Setup."""

import os
from pathlib import Path
import shutil
import subprocess
import sys
from zipfile import ZipFile

from component_archive import json_object, sha256
from pinned_setup_download import fetch


REPO = Path(__file__).resolve().parent.parent
COMPONENT = REPO / "components/openrgb"
OPENRGB_UPSTREAM = "components/openrgb/upstream"
POWERSHELL = Path(os.environ.get("SystemRoot", "C:/Windows")) / "System32/WindowsPowerShell/v1.0/powershell.exe"
BUNDLE = "Prismestra-OpenRGB-component.zip"
PINNED_INPUTS = {
    "jom.zip": (
        "https://download.qt.io/official_releases/jom/jom_1_1_4.zip",
        "d533c1ef49214229681e90196ed2094691e8c4a0a0bef0b2c901debcb562682b",
        1696930,
    ),
    "qtbase.7z": (
        "https://download.qt.io/online/qtsdkrepository/windows_x86/desktop/qt6_683/qt6_683/qt.qt6.683.win64_msvc2022_64/"
        "6.8.3-0-202503201308qtbase-Windows-Windows_11_23H2-MSVC2022-Windows-Windows_11_23H2-X86_64.7z",
        "41688269fac0565db956c66d9eecae777d16197e0c02cd81b88640c1f5d73d3f",
        39484220,
    ),
}
CRT_FILES = ("msvcp140.dll", "msvcp140_codecvt_ids.dll", "vcruntime140.dll", "vcruntime140_1.dll")


def run(*args, cwd=REPO, env=None):
    subprocess.run(args, cwd=cwd, env=env, check=True)


def recipe(name, *args):
    run(sys.executable, REPO / "scripts" / name, *args)


def read(path):
    return json_object(path.read_bytes())


def git_output(path, *args):
    return subprocess.check_output(["git", "-C", path, *args], encoding="utf-8").strip()


def verify_openrgb_gitlink(repo):
    lock = read(repo / "components/openrgb/source-lock.json")
    revision = lock["revision"]
    link = git_output(repo, "ls-tree", "HEAD", "--", OPENRGB_UPSTREAM)
    if link != f"160000 commit {revision}\t{OPENRGB_UPSTREAM}":
        raise ValueError("Committed OpenRGB gitlink differs from its source lock")
    for key, expected in (("path", OPENRGB_UPSTREAM), ("url", lock["repository"])):
        values = git_output(repo, "config", "--blob", "HEAD:.gitmodules", "--get-all",
                            f"submodule.openrgb-headless.{key}")
        if values != expected:
            raise ValueError(f"Committed OpenRGB submodule {key} differs from its source lock")
    return revision


def checkout(lock, path):
    run("git", "init", "-q", path)
    run("git", "-C", path, "config", "core.autocrlf", "true")
    run("git", "-C", path, "remote", "add", "origin", lock["repository"])
    run("git", "-C", path, "fetch", "--depth=1", "origin", lock["revision"])
    run("git", "-C", path, "checkout", "--detach", lock["revision"])
    return path


def checkout_openrgb(repo, path):
    revision = verify_openrgb_gitlink(repo)
    upstream = repo / OPENRGB_UPSTREAM
    if (not (upstream / ".git").exists()
            or Path(git_output(upstream, "rev-parse", "--show-toplevel")).resolve() != upstream.resolve()
            or git_output(upstream, "rev-parse", "--verify", "HEAD") != revision):
        raise ValueError(f"Initialize the pinned OpenRGB checkout with git submodule update --init -- {OPENRGB_UPSTREAM}")
    # Fetch only the committed objects into an independent checkout. The build
    # cannot consume changes, ignored files or generated objects in the submodule.
    return checkout({"repository": upstream, "revision": revision}, path)


def apply_locked_patch(source, patch, expected_sha256):
    from components.verify_openrgb_component import working_diff

    if sha256(patch) != expected_sha256:
        raise ValueError("OpenRGB patch differs from its source lock")
    run("git", "-C", source, "apply", "--check", patch)
    run("git", "-C", source, "apply", patch)
    run("git", "-C", source, "add", "--intent-to-add", "--all")
    if working_diff(source) != patch.read_bytes():
        raise ValueError(
            "OpenRGB patch differs from the applied Git diff; "
            "regenerate with git diff --binary --full-index HEAD --"
        )


def stage_pinned_hidapi(runtime, archives, dependency_lock, dependencies, root, vsdevcmd):
    from components.build_hidapi import build
    from components.package_openrgb_component import hidapi_patch

    patch = hidapi_patch(dependency_lock, dependencies)
    if patch is None:
        return
    pins = dependencies["files"]["hidapi-hotplug.dll"]
    archive = archives / f"hidapi-hotplug-{pins['source_revision']}-source.zip"
    rebuilt = build(archive, patch, pins, root / "hidapi-build", vsdevcmd)
    shutil.copy2(rebuilt, runtime / "hidapi-hotplug.dll")


def stage_sources(archives, assets):
    from components.package_openrgb_component import safe_name

    archives.mkdir()
    for entry in read(assets)["files"]:
        path = safe_name(entry["path"])
        if path.parent.as_posix() == "source":
            fetch(archives, path.name, entry["url"], entry["sha256"], entry["size"])


def stage_runtime(source, runtime, environment):
    from components.verify_openrgb_component import SOURCE_FILES

    runtime.mkdir()
    for name, path in SOURCE_FILES.items():
        if name != "PawnIOLib.dll":
            shutil.copy2(source / path, runtime / name)
    redist = Path(environment["VCTOOLSREDISTDIR"]) / "x64/Microsoft.VC143.CRT"
    for name in CRT_FILES:
        shutil.copy2(redist / name, runtime / name)


def stage_pinned_pawnio(runtime, archives, pins, work, vsdevcmd, environment):
    from components.build_hidapi import extract_source

    stem = f"PawnIO-{pins['source_revision']}"
    archive = archives / f"{stem}-source.zip"
    if sha256(archive) != pins["source_archive_sha256"]:
        raise ValueError("PawnIO source archive differs from the dependency lock")
    toolset = environment["VCTOOLSVERSION"].rstrip("\\/")
    sdk = environment["WINDOWSSDKVERSION"].rstrip("\\/")
    if (toolset, sdk) != (pins["msvc_toolset"], pins["windows_sdk"]):
        raise ValueError(f"PawnIO toolchain differs from the dependency lock: MSVC {toolset}, SDK {sdk}")
    source = work / "source"
    extract_source(archive, source, stem)
    tools = vsdevcmd.parents[2] / "Common7/IDE/CommonExtensions/Microsoft/CMake"
    cmake = tools / "CMake/bin/cmake.exe"
    ninja = tools / "Ninja/ninja.exe"
    output = work / "build"
    run(cmake, "-S", REPO / "scripts/components/pawniolib", "-B", output, "-G", "Ninja",
        f"-DCMAKE_MAKE_PROGRAM={ninja}", "-DCMAKE_BUILD_TYPE=Release",
        f"-DPAWN_SOURCE:PATH={source}", env=environment)
    run(cmake, "--build", output, "--target", "PawnIOLib", "--verbose", env=environment)
    shutil.copy2(output / "PawnIOLib.dll", runtime / "PawnIOLib.dll")


def build_component(root, vsdevcmd):
    from components.build_hidapi import msvc_environment

    source_lock = COMPONENT / "source-lock.json"
    dependency_lock = COMPONENT / "dependency-lock.json"
    lock = read(source_lock)
    patch = COMPONENT / lock["patch"]
    source = checkout_openrgb(REPO, root / "openrgb-source")
    apply_locked_patch(source, patch, lock["patch_sha256"])
    tools = {name: fetch(root, name, *PINNED_INPUTS[name])
             for name in PINNED_INPUTS}

    runtime = root / "runtime"
    archives = root / "component-sources"
    assets = COMPONENT / "distribution-assets.json"
    stage_sources(archives, assets)
    environment = msvc_environment(vsdevcmd)
    dependencies = read(dependency_lock)
    stage_runtime(source, runtime, environment)
    stage_pinned_pawnio(runtime, archives, dependencies["files"]["PawnIOLib.dll"],
                       root / "pawnio-build", vsdevcmd, environment)
    stage_pinned_hidapi(runtime, archives, dependency_lock, dependencies, root, vsdevcmd)

    jom = root / "jom"
    with ZipFile(tools["jom.zip"]) as archive:
        jom.mkdir()
        (jom / "jom.exe").write_bytes(archive.read("jom.exe"))
    qt = root / "qt"
    seven_zip = Path(os.environ["ProgramFiles"]) / "7-Zip/7z.exe"
    run(seven_zip, "x", "-y", f"-o{qt}", tools["qtbase.7z"])
    (qmake,) = qt.rglob("qmake.exe")
    build = root / "openrgb-build"
    run(POWERSHELL, "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
        "-File", REPO / "scripts/components/build_openrgb.ps1", "-SourceRoot", source,
        "-BuildRoot", build, "-QtRoot", qmake.parent.parent, "-Jom", jom / "jom.exe",
        "-VsDevCmd", vsdevcmd)
    shutil.copy2(build / "release/OpenRGB.exe", runtime)
    inventory = root / "openrgb-inventory.json"
    recipe("components/verify_openrgb_component.py", "--source-root", source, "--release-dir", runtime,
           "--source-lock", source_lock, "--dependency-lock", dependency_lock, "--output", inventory)
    bundle = root / BUNDLE
    recipe("components/package_openrgb_component.py", "--runtime-dir", runtime, "--inventory", inventory,
           "--source-lock", source_lock, "--dependency-lock", dependency_lock, "--patch", patch,
           "--source-archives-dir", archives, "--licenses-dir", COMPONENT / "licenses",
           "--recipes-dir", REPO / "scripts/components", "--asset-manifest", assets,
           "--verification-doc", REPO / "docs/openrgb-component-verification.md", "--output", bundle)
    recipe("components/test_parent_pipe.py", "--exe", runtime / "OpenRGB.exe",
           "--sha256", sha256(runtime / "OpenRGB.exe"), "--artifacts", root / "component-parent-smoke")
    return bundle
