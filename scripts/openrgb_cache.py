"""Verify and reuse complete OpenRGB bundles from trusted Windows builds."""

import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import tempfile

from components.package_openrgb_component import LICENSES, RECIPES, checked, read_json
from components.verify_openrgb_component import MSVC_RUNTIME, REQUIRED, fingerprint
from package_installer import extract_component, read_prerequisite_lock


BUNDLE = "Prismestra-OpenRGB-component.zip"
RECEIPT = "openrgb-cache.json"
READY = "openrgb-cache-ready"
SHA256 = re.compile(r"[0-9a-f]{64}")
REPO = Path(__file__).resolve().parent.parent


def cache_key(inputs, image, toolchain, tools):
    if not SHA256.fullmatch(inputs) or not image or not toolchain or not tools:
        raise ValueError("OpenRGB cache requires exact source, image and toolchain inputs")
    selected_tools = {name: fingerprint(path) for name, path in tools.items()}
    data = json.dumps([inputs, image, toolchain, selected_tools], sort_keys=True).encode("utf-8")
    return "openrgb-windows-x64-v1-" + hashlib.sha256(data).hexdigest()


def verify_members(manifest, repo):
    component = repo / "components/openrgb"
    lock, _ = read_json(component / "source-lock.json")
    dependencies, _ = read_json(component / "dependency-lock.json")
    assets, _ = read_json(component / "distribution-assets.json")
    if (manifest["engine_revision"], manifest["engine_repository"]) != (lock["revision"], lock["repository"]):
        raise ValueError("Cached OpenRGB source differs from the checked source pin")
    expected = {
        **{f"build/{name}": fingerprint(component / name)
           for name in ("source-lock.json", "dependency-lock.json", lock["patch"])},
        **{f"build/{name}": fingerprint(repo / "scripts/components" / name) for name in RECIPES},
        "build/VERIFICATION.md": fingerprint(repo / "docs/openrgb-component-verification.md"),
        **{entry["path"]: {field: entry[field] for field in ("sha256", "size")}
           for entry in assets["files"] if entry["path"].partition("/")[0] in ("source", "licenses")},
    }
    if not {f"licenses/{name}" for name in LICENSES} <= expected.keys():
        raise ValueError("Missing OpenRGB notices in the checked distribution pins")
    required = {f"runtime/{name}" for name in REQUIRED} | expected.keys()
    allowed = required | {f"runtime/{name}" for name in MSVC_RUNTIME}
    entries = {row["path"]: row for row in manifest["files"]}
    if not required <= entries.keys() <= allowed:
        raise ValueError("Cached OpenRGB bundle is incomplete or has unexpected members")
    for path, pin in expected.items():
        if any(entries[path][field] != value for field, value in pin.items()):
            raise ValueError(f"Cached OpenRGB member differs from the checked input: {path}")
    for name, pin in dependencies["files"].items():
        if entries[f"runtime/{name}"]["sha256"] != pin["sha256"]:
            raise ValueError(f"Cached OpenRGB dependency differs from the checked pin: {name}")


def restore(cache, key, work, repo=REPO):
    checked(cache, directory=True)
    if {path.name for path in cache.iterdir()} != {BUNDLE, RECEIPT}:
        raise ValueError("Unexpected contents of the OpenRGB cache")
    receipt_path = checked(cache / RECEIPT)
    if receipt_path.stat().st_size > 4096:
        raise ValueError("Unexpected OpenRGB cache receipt size")
    receipt, _ = read_json(receipt_path)
    digest = receipt["bundle_sha256"]
    if receipt["key"] != key or not SHA256.fullmatch(digest):
        raise ValueError("Cached OpenRGB receipt does not match the exact build key")
    bundle = checked(cache / BUNDLE)
    _, runtime_floor = read_prerequisite_lock(repo / "scripts/installer/prerequisites.lock.json")
    with tempfile.TemporaryDirectory(dir=work, prefix="validate-openrgb-") as scratch:
        staged = Path(scratch) / "component"
        # Verify ZIP layout, every member's digest and the AMD64 linker/runtime floor.
        extract_component(bundle, staged, digest, runtime_floor)
        manifest, _ = read_json(staged / "component-manifest.json")
        verify_members(manifest, repo)
    return bundle


def save(cache, key, bundle):
    checked(cache.parent, directory=True)
    cache.mkdir()
    shutil.copyfile(bundle, cache / BUNDLE)
    receipt = {"key": key, "bundle_sha256": fingerprint(bundle)["sha256"]}
    (cache / RECEIPT).write_text(json.dumps(receipt) + "\n", encoding="utf-8")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--inputs", required=True)
    parser.add_argument("--image", required=True)
    parser.add_argument("--toolchain", required=True)
    parser.add_argument("--tool", nargs=2, action="append", required=True, metavar=("NAME", "PATH"))
    args = parser.parse_args()
    print(cache_key(args.inputs, args.image, args.toolchain,
                    {name: Path(path) for name, path in args.tool}))


if __name__ == "__main__":
    main()
