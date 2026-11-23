"""Build the Windows CI candidate from checked source and pinned distribution inputs."""

import argparse
import os
from pathlib import Path
import re
import shutil
import time
import uuid
from zipfile import BadZipFile
from zlib import error as ZlibError

import openrgb_cache
from build_openrgb_component import POWERSHELL, REPO, build_component, checkout, read, recipe, run
from component_archive import sha256
from package_installer import WEBVIEW_PROBE
from package_release import retain_binary_zip
from pinned_setup_download import fetch


INSTALLER = REPO / "scripts/installer"
INNO_INPUT = (
    "https://github.com/jrsoftware/issrc/releases/download/is-6_7_4/innosetup-6.7.4.exe",
    "a549929319f9120716677c685535ca4671e4b937b5fdc87e2585f32d8eb30e8c",
    10604576,
)


def load_or_build_component(root, vsdevcmd, cache, key, *, write_cache=False):
    started = time.monotonic()
    existing = cache is not None and (cache.exists() or cache.is_symlink())
    bundle = None
    status = "disabled" if cache is None else "miss"
    if existing:
        try:
            bundle = openrgb_cache.restore(cache, key, root)
            status = "hit"
        except (OSError, ValueError, KeyError, TypeError, RuntimeError, BadZipFile, ZlibError) as error:
            print(f"OpenRGB cache rejected; rebuilding from pins: {error}", flush=True)
            status = "rejected"
    if bundle is None:
        bundle = build_component(root, vsdevcmd)
        if cache is not None and not existing and write_cache:
            try:
                openrgb_cache.save(cache, key, bundle)
                (root / openrgb_cache.READY).touch(exist_ok=False)
            except (OSError, ValueError) as error:
                print(f"OpenRGB component built; cache could not be staged: {error}", flush=True)
    print(f"OpenRGB cache {status}; verified component ready in {time.monotonic() - started:.1f}s"
          f" (SHA-256 {sha256(bundle)})", flush=True)
    return bundle


def package_candidate(root, commit, version, component, prerequisites, iscc, pawnio):
    release = root / "release"
    run("cargo", "fetch", "--locked")
    recipe("package_release.py", "--expected-commit", commit, "--output", release,
           "--include-upgrade-route-probe")
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
    retain_binary_zip(release, root / "portable", manifest)
    print(f"Compiled Setup SHA-256: {receipt['installer_sha256']}", flush=True)

    # These two builds exercise repair and version transitions in a disposable
    # namespace. The ordinary-user test uses the production EXE above.
    fixtures = root / "fixtures"
    namespace = str(uuid.uuid4())
    (fixtures / "first").mkdir(parents=True)
    route_probe = fixtures / "setup_upgrade_routes.exe"
    shutil.copyfile(release / "build/x86_64-pc-windows-msvc/release/examples" / route_probe.name, route_probe)
    (fixtures / "upgrade-routes.sha256").write_text(sha256(route_probe) + "\n", encoding="ascii")
    shutil.copyfile(production / "stage/prerequisites" / WEBVIEW_PROBE, fixtures / WEBVIEW_PROBE)
    (fixtures / "webview-probe.sha256").write_text(receipt["webview_probe_sha256"] + "\n", encoding="ascii")
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
    parser.add_argument("--version")
    parser.add_argument("--vsdevcmd", type=Path, required=True)
    parser.add_argument("--openrgb-cache", type=Path)
    parser.add_argument("--openrgb-cache-key")
    parser.add_argument("--write-openrgb-cache", action="store_true")
    parser.add_argument("--openrgb-only", action="store_true")
    args = parser.parse_args()
    if not re.fullmatch(r"[0-9a-f]{40}", args.commit):
        parser.error("expected a full checkout commit")
    if not args.openrgb_only and (not args.version
            or not re.fullmatch(r"(?:0|[1-9][0-9]{0,4})(?:\.(?:0|[1-9][0-9]{0,4})){3}", args.version)
            or args.version == "0.0.0.0" or any(int(part) > 65535 for part in args.version.split("."))):
        parser.error("expected a nonzero four-part installer version with 16-bit components")
    root = args.output.resolve()
    if root.is_relative_to(REPO):
        parser.error("output must be outside the checkout")
    cache = None if args.openrgb_cache is None else args.openrgb_cache.absolute()
    key = args.openrgb_cache_key
    if (cache is None) != (key is None) or (key is not None and not openrgb_cache.KEY.fullmatch(key)):
        parser.error("an OpenRGB cache directory and exact build key must be supplied together")
    if (args.write_openrgb_cache and cache is None) or (args.openrgb_only and not args.write_openrgb_cache):
        parser.error("OpenRGB warm-up requires an exact cache key and write access")
    if cache is not None and (cache.resolve().is_relative_to(REPO) or cache.resolve().is_relative_to(root)):
        parser.error("OpenRGB cache must be outside both the checkout and candidate output")
    from package_release import clean_commit

    clean_commit(REPO, args.commit)
    root.mkdir(parents=True)
    vsdevcmd = args.vsdevcmd.resolve(strict=True)
    if args.openrgb_only:
        component = load_or_build_component(root, vsdevcmd, cache, key, write_cache=True)
        if component != cache / openrgb_cache.BUNDLE:
            if not (root / openrgb_cache.READY).is_file():
                raise ValueError("OpenRGB cache could not be staged for warm-up")
            openrgb_cache.restore(cache, key, root)
        return
    inno_bootstrap = fetch(root, "innosetup-6.7.4.exe", *INNO_INPUT)
    prerequisites = root / "prerequisites"
    prerequisites.mkdir()
    for prerequisite, pin in read(INSTALLER / "prerequisites.lock.json")["packages"].items():
        url = os.environ.get("PRISMESTRA_" + prerequisite.upper() + "_URL") or pin["source"]
        fetch(prerequisites, pin["file"], url, pin["sha256"], pin["size"])
    inno = root / "inno"
    run(POWERSHELL, "-NoProfile", "-NonInteractive", "-Command",
        "$params = '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /DIR=' + [char]34 + "
        "$env:PRISMESTRA_INNO_DIRECTORY + [char]34; "
        "$p = Start-Process -FilePath $env:PRISMESTRA_INNO_BOOTSTRAP "
        "-Wait -PassThru -WindowStyle Hidden -ArgumentList $params; exit $p.ExitCode",
        env=dict(os.environ, PRISMESTRA_INNO_BOOTSTRAP=str(inno_bootstrap),
                 PRISMESTRA_INNO_DIRECTORY=str(inno)))
    component = load_or_build_component(root, vsdevcmd, cache, key, write_cache=args.write_openrgb_cache)
    pawnio = checkout(read(INSTALLER / "pawnio-source.lock.json"), root / "pawnio-source")
    run("git", "-C", pawnio, "submodule", "update", "--init", "--recursive", "--depth=1")
    package_candidate(root, args.commit, args.version, component, prerequisites, inno / "ISCC.exe", pawnio)


if __name__ == "__main__":
    main()
