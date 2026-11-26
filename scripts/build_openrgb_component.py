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
    "Prismestra-Sources-0.1.0.5.zip": (
        "https://github.com/376east/prismestra/releases/download/build-inputs-v1/Prismestra-Sources-0.1.0.5.zip",
        "a3ed186b4d08cf25930689902a0f1d260cc3eca678379d897281a51bc2b0307e",
        55900191,
    ),
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
    run("git", "-C", source, "apply", "--intent-to-add", patch)
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


def build_component(root, vsdevcmd):
    from components.verify_openrgb_component import REQUIRED, MSVC_RUNTIME
    source_lock = COMPONENT / "source-lock.json"
    dependency_lock = COMPONENT / "dependency-lock.json"
    lock = read(source_lock)
    patch = COMPONENT / lock["patch"]
    source = checkout_openrgb(REPO, root / "openrgb-source")
    apply_locked_patch(source, patch, lock["patch_sha256"])
    tools = {name: fetch(root, name, *PINNED_INPUTS[name])
             for name in PINNED_INPUTS}

    # Reuse the pinned source archives and unchanged user-mode dependencies.
    # The previous OpenRGB.exe is deliberately excluded from the new component.
    with ZipFile(tools["Prismestra-Sources-0.1.0.5.zip"]) as previous:
        base = root / "previous-component.zip"
        base.write_bytes(previous.read("openrgb/Prismestra-OpenRGB-component.zip"))
    if sha256(base) != "42ab552ffb4d429a9df7d83aab8d53a1df30269491f2e28c9dd57c7d9187f8e7":
        raise ValueError("Unexpected dependency baseline component")
    runtime = root / "runtime"
    archives = root / "component-sources"
    runtime.mkdir()
    archives.mkdir()
    assets = COMPONENT / "distribution-assets.json"
    with ZipFile(base) as previous:
        for name in sorted((REQUIRED - {"OpenRGB.exe"}) | MSVC_RUNTIME):
            member = f"runtime/{name}"
            if member in previous.namelist():
                (runtime / name).write_bytes(previous.read(member))
        for entry in read(assets)["files"]:
            role, name = entry["path"].split("/")
            if role == "source":
                (archives / name).write_bytes(previous.read(entry["path"]))
    stage_pinned_hidapi(runtime, archives, dependency_lock, read(dependency_lock), root, vsdevcmd)

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
