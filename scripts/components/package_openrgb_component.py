"""Package pinned OpenRGB inputs without building, installing or executing them.

The static verifier report and distribution asset manifest are caller-selected
trust inputs. Packaging does not establish license approval or runtime behavior.
"""
import argparse
import hashlib
import json
import re
import stat
from pathlib import Path, PurePosixPath
import zipfile

if __package__:
    from . import verify_openrgb_component as verifier
else:
    import verify_openrgb_component as verifier


RECIPES = ("build_openrgb.ps1", "build_openrgb_dependencies.ps1",
           "verify_openrgb_component.py", "test_verify_openrgb_component.py",
           "test_actual_openrgb.py", "test_parent_pipe.py", "pawniolib/CMakeLists.txt")
LICENSES = {"OpenRGB-GPL-2.0.txt", "LICENSE-bsd.txt", "LICENSE-orig.txt", "LICENSE.txt",
            "LGPL-2.1.txt", "PawnIO-COPYING", "PawnIO-Modules-LGPL-2.1.txt", "libusb-LGPL-2.1.txt"}


def safe_name(name):
    if not isinstance(name, str) or not name or "\\" in name:
        raise ValueError("invalid bundle path")
    parts = name.split("/")
    reserved = {"CON", "PRN", "AUX", "NUL", *[f"{prefix}{i}" for prefix in ("COM", "LPT") for i in range(1, 10)]}
    if any(not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._+-]*", p) or p.endswith(".")
           or p.split(".")[0].upper() in reserved for p in parts):
        raise ValueError("unsafe bundle path: " + name)
    return PurePosixPath(name)


def checked(path, directory=False):
    path = Path(path).absolute()
    for ancestor in (path, *path.parents):
        info = ancestor.lstat()
        if stat.S_ISLNK(info.st_mode) or getattr(info, "st_file_attributes", 0) & 0x400:
            raise ValueError(f"symlink/reparse input: {ancestor}")
    if directory and not path.is_dir():
        raise ValueError(f"directory required: {path}")
    return path


def read_json(path):
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError("duplicate JSON key: " + key)
            result[key] = value
        return result
    data = checked(path).read_bytes()
    value = json.loads(data.decode("utf-8-sig"), object_pairs_hook=unique)
    if not isinstance(value, dict):
        raise ValueError("JSON object required")
    return value, {"size": len(data), "sha256": hashlib.sha256(data).hexdigest()}


def metadata(item):
    if (not isinstance(item, dict) or type(item.get("size")) is not int or item["size"] <= 0
            or not re.fullmatch(r"[0-9a-f]{64}", str(item.get("sha256", "")))):
        raise ValueError("invalid pinned file metadata")
    return {key: item[key] for key in ("size", "sha256")}


def package(args):
    runtime = checked(args.runtime_dir, directory=True)
    sources = checked(args.source_archives_dir, directory=True)
    licenses = checked(args.licenses_dir, directory=True)
    recipes = checked(args.recipes_dir, directory=True)
    output = Path(args.output).absolute()
    safe_name(output.name)
    if output.suffix.lower() != ".zip":
        raise ValueError("output must be a ZIP file")
    checked(output.parent, directory=True)
    if output.exists() or output.is_symlink():
        raise ValueError("output already exists; refusing overwrite")
    for root in (runtime, sources, licenses, recipes):
        if output.is_relative_to(root):
            raise ValueError("output must be outside input directories")
    (lock, lock_info), (dependencies, dependency_info) = read_json(args.source_lock), read_json(args.dependency_lock)
    (inventory, _), (assets, _) = read_json(args.inventory), read_json(args.asset_manifest)
    if (type(lock.get("version")) is not int or lock["version"] != 1 or lock.get("revision") != verifier.SOURCE_HEAD
            or not isinstance(lock.get("repository"), str) or not lock["repository"].startswith("https://")
            or lock.get("untracked_source_paths") != []):
        raise ValueError("invalid pinned source lock")
    patch_name = safe_name(lock.get("patch"))
    if len(patch_name.parts) != 1 or patch_name.suffix != ".patch":
        raise ValueError("source-lock patch must be a basename")
    if (type(inventory.get("schema_version")) is not int or inventory["schema_version"] != 1
            or inventory.get("scope") != "static_component_candidate"
            or inventory.get("ok") is not True or inventory.get("errors") != []
            or inventory.get("source_head") != lock["revision"]
            or inventory.get("source_patch_locked") is not True
            or inventory.get("untracked_paths") != []
            or inventory.get("tracked_patch", {}).get("sha256") != lock.get("patch_sha256")):
        raise ValueError("matching successful static verifier inventory required")
    if (type(dependencies.get("version")) is not int or dependencies["version"] != 1
            or not isinstance(dependencies.get("files"), dict)
            or not {"PawnIOLib.dll", "hidapi-hotplug.dll"} <= dependencies["files"].keys()):
        raise ValueError("invalid dependency lock")
    if type(assets.get("version")) is not int or assets["version"] != 1 or not isinstance(assets.get("files"), list):
        raise ValueError("invalid distribution asset manifest")

    files, seen = {}, set()
    def add(source, name, expected=None):
        safe_name(name)
        if name.casefold() in seen:
            raise ValueError("duplicate bundle path: " + name)
        seen.add(name.casefold())
        source = checked(source)
        info = verifier.fingerprint(source)
        if expected is not None and info != metadata(expected):
            raise ValueError("pinned hash/size mismatch: " + name)
        files[name] = (source, info)

    add(args.source_lock, "build/source-lock.json", lock_info)
    add(args.dependency_lock, "build/dependency-lock.json", dependency_info)
    add(args.patch, "build/" + str(patch_name))
    if files["build/" + str(patch_name)][1]["sha256"] != lock.get("patch_sha256"):
        raise ValueError("source patch SHA256 mismatch")
    names = set()
    for item in inventory.get("files", []):
        name = str(safe_name(item["name"]))
        if "/" in name:
            raise ValueError("runtime inventory requires basenames")
        names.add(name.lower())
        add(runtime / name, "runtime/" + name, item)
    allowed = {n.lower() for n in verifier.REQUIRED} | verifier.MSVC_RUNTIME
    if (not {n.lower() for n in verifier.REQUIRED} <= names or not names <= allowed
            or {p.name for p in runtime.iterdir()} != {PurePosixPath(n).name for n in files if n.startswith("runtime/")}):
        raise ValueError("runtime directory/inventory differs from verifier allowlist")

    asset_names = set()
    for item in assets["files"]:
        name = str(safe_name(item["path"]))
        if name.casefold() in asset_names:
            raise ValueError("duplicate distribution asset path")
        asset_names.add(name.casefold())
        role, _, basename = name.partition("/")
        if role in ("source", "licenses"):
            if "/" in basename or not basename:
                raise ValueError("distribution assets require basenames")
            add((sources if role == "source" else licenses) / basename, name, item)
    source_names = {PurePosixPath(n).name for n in files if n.startswith("source/")}
    if (f"openrgb-headless-{lock['revision']}-source.zip" not in source_names
            or "libusb-1.0.27-source.tar.bz2" not in source_names
            or not any(re.fullmatch(r"PawnIO.Modules-[0-9a-f]{40}-source.zip", n) for n in source_names)
            or not {"licenses/" + n for n in LICENSES} <= files.keys()):
        raise ValueError("required pinned upstream/module sources or notices missing")
    source_hashes = {info["sha256"] for name, (_, info) in files.items() if name.startswith("source/")}
    for name, item in dependencies["files"].items():
        if (name not in ("PawnIOLib.dll", "hidapi-hotplug.dll", "libusb-1.0.dll")
                or not re.fullmatch(r"[0-9a-f]{40}", str(item.get("source_revision", "")))
                or item.get("source_archive_sha256") not in source_hashes
                or files.get("runtime/" + name, (None, {}))[1].get("sha256") != item.get("sha256")):
            raise ValueError("rebuilt dependency binary/source pins mismatch: " + name)
    for name in RECIPES:
        add(recipes / name, "build/" + name)
    if args.verification_doc:
        add(args.verification_doc, "build/VERIFICATION.md")

    manifest = {"version": 1, "scope": "packaging_only", "engine_repository": lock["repository"],
                "engine_revision": lock["revision"], "engine_sha256": files["runtime/OpenRGB.exe"][1]["sha256"],
                "unverified": ["license approval", "complete corresponding source", "binary built from this source",
                               "runtime and hardware behavior", "signing", "clean Windows compatibility"],
                "files": [{"path": name, **info} for name, (_, info) in sorted(files.items())]}
    encoded = (json.dumps(manifest, ensure_ascii=False, indent=2) + "\n").encode("utf-8")
    # Exclusive creation protects an existing output, including a concurrent creator.
    with output.open("xb") as stream, zipfile.ZipFile(stream, "w", zipfile.ZIP_DEFLATED, compresslevel=6) as archive:
        for name, (source, _) in sorted(files.items()):
            archive.write(source, name)
        archive.writestr("component-manifest.json", encoded)
    with zipfile.ZipFile(output) as archive:
        if archive.read("component-manifest.json") != encoded:
            raise ValueError("bundle manifest changed during packaging")
        for name, (_, info) in files.items():
            with archive.open(name) as stream:
                digest = hashlib.file_digest(stream, "sha256").hexdigest()  # Also checks ZIP CRC at EOF.
            if archive.getinfo(name).file_size != info["size"] or digest != info["sha256"]:
                raise ValueError("bundle payload changed during packaging: " + name)
    return {"scope": "packaging_only", "files": len(files) + 1, "crc_and_manifest": "PASS",
            "native_hardware_activated": False, **verifier.fingerprint(output)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("runtime-dir", "inventory", "source-lock", "dependency-lock", "patch",
                 "source-archives-dir", "licenses-dir", "recipes-dir", "asset-manifest", "output"):
        parser.add_argument("--" + name, required=True, type=Path)
    parser.add_argument("--verification-doc", type=Path)
    args = parser.parse_args()
    try:
        result = package(args)
    except (OSError, ValueError, KeyError, TypeError, zipfile.BadZipFile) as error:
        parser.exit(1, str(error) + "\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
