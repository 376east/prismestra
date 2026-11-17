"""Combine the exact app, backend and signed driver's corresponding sources."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import zipfile


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def git(root, *args):
    return subprocess.check_output(["git", "-C", str(root), *args]).decode().strip()


def tracked(root, prefix=""):
    if git(root, "status", "--porcelain", "--untracked-files=all"):
        raise ValueError("Driver source tree is not clean")
    for name in git(root, "ls-files", "-z").split("\0"):
        if not name:
            continue
        source = root / name
        if source.is_symlink():
            raise ValueError("Source symlinks are not accepted")
        if source.is_dir():
            yield from tracked(source, prefix + name + "/")
        elif source.is_file():
            yield source, prefix + name
        else:
            raise ValueError("Missing tracked source file")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app-source", required=True, type=Path)
    parser.add_argument("--component", required=True, type=Path)
    parser.add_argument("--pawnio-source-tree", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    root = args.pawnio_source_tree.resolve(strict=True)
    lock = json.loads((Path(__file__).parent / "installer/pawnio-source.lock.json").read_bytes())
    if git(root, "rev-parse", "HEAD") != lock["revision"]:
        raise ValueError("PawnIO source differs from the driver pin")
    for name, revision in lock["submodules"].items():
        if git(root / name, "rev-parse", "HEAD") != revision:
            raise ValueError("PawnIO submodule differs from its pin")
    for path in (args.app_source, args.component):
        if path.is_symlink() or not path.is_file() or path.suffix != ".zip":
            raise ValueError("Expected verified input ZIP archives")
        with zipfile.ZipFile(path) as source:
            if source.testzip() is not None:
                raise ValueError("Source archive CRC failed")
    files = list(tracked(root))
    manifest = {"app_source": {"file": args.app_source.name, "sha256": digest(args.app_source)},
                "openrgb_component": {"file": args.component.name, "sha256": digest(args.component)},
                "pawnio": {**lock, "files": [{"path": name, "sha256": digest(path)} for path, name in files]}}
    with args.output.open("xb") as stream, zipfile.ZipFile(stream, "w", zipfile.ZIP_DEFLATED, compresslevel=6) as archive:
        archive.write(args.app_source, "application/" + args.app_source.name)
        archive.write(args.component, "openrgb/" + args.component.name)
        for path, name in files:
            archive.write(path, "pawnio-2.2.0/" + name)
        archive.writestr("source-manifest.json", json.dumps(manifest, indent=2) + "\n")
        archive.writestr("README.txt", "Prismestra corresponding source bundle.\napplication/: exact application, Cargo dependencies and licenses.\nopenrgb/: private backend binary/source pins, upstream sources, patch, recipes and licenses.\npawnio-2.2.0/: source and pinned PawnPP submodule for the unmodified official signed driver installer. Driver signing credentials are not distributed.\nMicrosoft VC++ and WebView2 installers are unmodified proprietary runtime prerequisites, not linked application source.\n")
    print(json.dumps({"file": args.output.name, "sha256": digest(args.output), "bytes": args.output.stat().st_size}))


if __name__ == "__main__":
    main()
