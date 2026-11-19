"""Build the Windows CI candidate from checked source and pinned distribution inputs."""

import argparse
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
from urllib.parse import urlsplit
from urllib.request import urlopen
import uuid
from zipfile import ZipFile

from package_release import sha256
from package_installer import json_object

REPO = Path(__file__).resolve().parent.parent
COMPONENT = REPO / "components/openrgb"
INSTALLER = REPO / "scripts/installer"
POWERSHELL = Path(os.environ.get("SystemRoot", "C:/Windows")) / "System32/WindowsPowerShell/v1.0/powershell.exe"
PINNED_INPUTS = {
    "Prismestra-Sources-0.1.0.5.zip": (
        "https://github.com/malyarq/prismestra/releases/download/v0.1.0-preview.5/Prismestra-Sources-0.1.0.5.zip",
        "a3ed186b4d08cf25930689902a0f1d260cc3eca678379d897281a51bc2b0307e",
        55900191,
    ),
    "innosetup-6.7.4.exe": (
        "https://github.com/jrsoftware/issrc/releases/download/is-6_7_4/innosetup-6.7.4.exe",
        "a549929319f9120716677c685535ca4671e4b937b5fdc87e2585f32d8eb30e8c",
        10604576,
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


def fetch(directory, name, url, digest, size):
    destination = directory / name
    parsed = urlsplit(url)
    if parsed.scheme != "https" or parsed.username or parsed.password or not parsed.hostname:
        raise ValueError(f"{name}: expected an HTTPS download URL without credentials")
    print(f"Fetching pinned {name} from {parsed.hostname}", flush=True)
    with urlopen(url, timeout=90) as response, destination.open("xb") as output:
        if urlsplit(response.url).scheme != "https":
            raise ValueError(f"{name}: download redirected away from HTTPS")
        remaining = size
        while chunk := response.read(min(1024 * 1024, remaining + 1)):
            remaining -= len(chunk)
            if remaining < 0:
                raise ValueError(f"{name}: download exceeded pinned size")
            output.write(chunk)
    if destination.stat().st_size != size or sha256(destination) != digest:
        raise ValueError(f"{name}: download differs from pinned size/SHA-256")
    return destination


def checkout(lock, path):
    run("git", "init", "-q", path)
    run("git", "-C", path, "config", "core.autocrlf", "true")
    run("git", "-C", path, "remote", "add", "origin", lock["repository"])
    run("git", "-C", path, "fetch", "--depth=1", "origin", lock["revision"])
    run("git", "-C", path, "checkout", "--detach", lock["revision"])
    return path


def build_component(root, tools, vsdevcmd):
    from components.verify_openrgb_component import REQUIRED, MSVC_RUNTIME

    source_lock = COMPONENT / "source-lock.json"
    dependency_lock = COMPONENT / "dependency-lock.json"
    lock = read(source_lock)
    source = checkout(lock, root / "openrgb-source")
    patch = COMPONENT / lock["patch"]
    if sha256(patch) != lock["patch_sha256"]:
        raise ValueError("OpenRGB patch differs from its source lock")
    run("git", "-C", source, "apply", "--check", patch)
    run("git", "-C", source, "apply", "--intent-to-add", patch)

    # Reuse the published, pinned user-mode dependency bytes and source archives.
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
    bundle = root / "Prismestra-OpenRGB-component.zip"
    recipe("components/package_openrgb_component.py", "--runtime-dir", runtime, "--inventory", inventory,
           "--source-lock", source_lock, "--dependency-lock", dependency_lock, "--patch", patch,
           "--source-archives-dir", archives, "--licenses-dir", COMPONENT / "licenses",
           "--recipes-dir", REPO / "scripts/components", "--asset-manifest", assets,
           "--verification-doc", REPO / "docs/openrgb-component-verification.md", "--output", bundle)
    recipe("components/test_parent_pipe.py", "--exe", runtime / "OpenRGB.exe",
           "--sha256", sha256(runtime / "OpenRGB.exe"), "--artifacts", root / "component-parent-smoke")
    return bundle


def package_candidate(root, commit, version, component, prerequisites, iscc, pawnio):
    release = root / "release"
    run("cargo", "fetch", "--locked")
    recipe("package_release.py", "--expected-commit", commit, "--output", release)
    binary = release / "binary/Prismestra"
    manifest = read(binary / "release-manifest.json")
    common = ["--app-exe", binary / "Prismestra.exe", "--app-sha256", manifest["executable_sha256"],
              "--component-zip", component, "--component-sha256", sha256(component),
              "--expected-commit", commit, "--iscc", iscc]
    production = root / "production"
    candidate = root / "candidate"
    candidate.mkdir()
    recipe("package_installer.py", *common, "--version", version, "--output", production,
           "--release-dir", binary, "--prerequisite-lock", INSTALLER / "prerequisites.lock.json",
           "--prerequisite-dir", prerequisites)
    receipt = read(production / "installer-receipt.json")
    setup = production / receipt["installer"]
    shutil.copyfile(setup, candidate / setup.name)
    shutil.copyfile(production / "stage/payload/install-manifest.json", candidate / "install-manifest.json")
    sources = candidate / f"Prismestra-Sources-{version}.zip"
    recipe("package_installer_source.py", "--app-source", release / manifest["source_archive"],
           "--component", component, "--pawnio-source-tree", pawnio, "--output", sources)
    (candidate / "SHA256SUMS.txt").write_text(
        "".join(f"{sha256(path)}  {path.name}\n" for path in sorted(candidate.iterdir())), encoding="utf-8")
    print(f"Compiled Setup SHA-256: {receipt['installer_sha256']}", flush=True)

    # These two builds exercise repair and version transitions in a disposable
    # namespace. The ordinary-user test uses the production EXE above.
    fixtures = root / "fixtures"
    namespace = str(uuid.uuid4())
    (fixtures / "first").mkdir(parents=True)
    (fixtures / "namespace.txt").write_text(namespace + "\n", encoding="ascii")
    for name, fixture_version in (("first", "0.0.0.0"), ("upgrade", version)):
        build = root / f"fixture-{name}"
        recipe("package_installer.py", *common, "--version", fixture_version, "--output", build,
               "--test-namespace", namespace)
        receipt = read(build / "installer-receipt.json")
        for relative in ("installer-receipt.json", "stage/payload/install-manifest.json", receipt["installer"]):
            destination = fixtures / name / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(build / relative, destination)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--commit", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--vsdevcmd", type=Path, required=True)
    args = parser.parse_args()
    if not re.fullmatch(r"[0-9a-f]{40}", args.commit):
        parser.error("expected a full checkout commit")
    if (not re.fullmatch(r"(?:0|[1-9][0-9]{0,4})(?:\.(?:0|[1-9][0-9]{0,4})){3}", args.version)
            or args.version == "0.0.0.0" or any(int(part) > 65535 for part in args.version.split("."))):
        parser.error("expected a nonzero four-part installer version with 16-bit components")
    root = args.output.resolve()
    if root.is_relative_to(REPO):
        parser.error("output must be outside the checkout")
    from package_release import clean_commit

    clean_commit(REPO, args.commit)
    root.mkdir(parents=True)
    tools = {name: fetch(root, name, *pin) for name, pin in PINNED_INPUTS.items()}
    prerequisites = root / "prerequisites"
    prerequisites.mkdir()
    for key, pin in read(INSTALLER / "prerequisites.lock.json")["packages"].items():
        url = os.environ.get("PRISMESTRA_" + key.upper() + "_URL") or pin["source"]
        fetch(prerequisites, pin["file"], url, pin["sha256"], pin["size"])
    inno = root / "inno"
    run(POWERSHELL, "-NoProfile", "-NonInteractive", "-Command",
        "$params = '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /DIR=' + [char]34 + "
        "$env:PRISMESTRA_INNO_DIRECTORY + [char]34; "
        "$p = Start-Process -FilePath $env:PRISMESTRA_INNO_BOOTSTRAP "
        "-Wait -PassThru -WindowStyle Hidden -ArgumentList $params; exit $p.ExitCode",
        env=dict(os.environ, PRISMESTRA_INNO_BOOTSTRAP=str(tools["innosetup-6.7.4.exe"]),
                 PRISMESTRA_INNO_DIRECTORY=str(inno)))
    component = build_component(root, tools, args.vsdevcmd.resolve(strict=True))
    pawnio = checkout(read(INSTALLER / "pawnio-source.lock.json"), root / "pawnio-source")
    run("git", "-C", pawnio, "submodule", "update", "--init", "--recursive", "--depth=1")
    package_candidate(root, args.commit, args.version, component, prerequisites, inno / "ISCC.exe", pawnio)


if __name__ == "__main__":
    main()
