"""Validate and extract the exact Windows OpenRGB component archive."""

import hashlib
import json
from pathlib import PurePosixPath
import re
import shutil
import stat
import struct
import zipfile


BACKEND = "components/openrgb/runtime/OpenRGB.exe"
SHA256 = re.compile(r"[0-9a-f]{64}")
MAX_COMPONENT_BYTES = 1 << 30


def sha256(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def unique_pairs(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"Duplicate JSON field: {key}")
        result[key] = value
    return result


def json_object(data):
    value = json.loads(data, object_pairs_hook=unique_pairs)
    if not isinstance(value, dict):
        raise ValueError("Expected a JSON object")
    return value


def file_pin(path, expected):
    if not isinstance(expected, str) or not SHA256.fullmatch(expected):
        raise ValueError(f"Invalid SHA-256 pin for {path.name}")
    if path.is_symlink() or not path.is_file() or sha256(path) != expected:
        raise ValueError(f"Missing, unsafe, or unpinned file: {path.name}")


def check_amd64_executable(stream, name, runtime_floor=None, *, require_gui=False):
    header = stream.read(64)
    if len(header) != 64 or header[:2] != b"MZ":
        raise ValueError(f"Missing PE header: {name}")
    offset, = struct.unpack_from("<I", header, 60)
    stream.seek(offset)
    pe = stream.read(28)
    if pe[:6] != b"PE\0\0\x64\x86":
        raise ValueError(f"Expected an AMD64 executable: {name}")
    if require_gui:
        stream.seek(offset + 24 + 68)
        if (len(pe) < 28 or struct.unpack_from("<H", pe, 20)[0] < 70
                or pe[24:26] != b"\x0b\x02" or stream.read(2) != b"\x02\x00"):
            raise ValueError(f"Expected a PE32+ Windows GUI executable: {name}")
    if runtime_floor is not None:
        # PE signature + 20-byte COFF header precede the PE32+ optional
        # header's magic and MajorLinkerVersion / MinorLinkerVersion bytes.
        if len(pe) < 28 or struct.unpack_from("<H", pe, 20)[0] < 4 or pe[24:26] != b"\x0b\x02":
            raise ValueError(f"Missing PE32+ linker version: {name}")
        linker = tuple(pe[26:28])
        if linker > runtime_floor[:2]:
            raise ValueError(f"{name} linker {linker[0]}.{linker[1]} exceeds minimum VC runtime "
                             f"{runtime_floor[0]}.{runtime_floor[1]}")


def safe_archive_name(name):
    if not isinstance(name, str):
        raise ValueError("Archive path must be a string")
    path = PurePosixPath(name)
    if not path.parts or name != path.as_posix() or any(
        (
            not re.fullmatch(r"[A-Za-z0-9_.()+ -]{1,170}", part)
            or part in (".", "..")
            or part.endswith((" ", "."))
            or part.split(".")[0].upper() in {"CON", "PRN", "AUX", "NUL", *(f"COM{i}" for i in range(1, 10)), *(f"LPT{i}" for i in range(1, 10))}
        )
        for part in path.parts
    ):
        raise ValueError(f"Unsafe archive path: {name!r}")
    return path


def extract_component(source, destination, expected, runtime_floor=None):
    file_pin(source, expected)
    with zipfile.ZipFile(source) as archive:
        infos = {}
        folded = set()
        for item in archive.infolist():
            path = safe_archive_name(item.filename)
            key = str(path).casefold()
            if key in folded or item.is_dir() or stat.S_ISLNK(item.external_attr >> 16):
                raise ValueError(f"Unsafe or repeated component file: {path}")
            if str(path) != "component-manifest.json" and (
                not ((len(path.parts) == 2 and path.parts[0] in {"runtime", "licenses", "source", "build"})
                     or (2 < len(path.parts) <= 4 and path.parts[0] == "build"))
            ):
                raise ValueError(f"Unexpected component directory: {path}")
            folded.add(key)
            infos[str(path)] = item
        total = sum(item.file_size for item in infos.values())
        if total > MAX_COMPONENT_BYTES or len(infos) > 4096:
            raise ValueError("Component archive exceeds staging limits")
        index = infos.pop("component-manifest.json")
        if index.file_size > 2 << 20:
            raise ValueError("Component manifest exceeds staging limit")
        manifest_raw = archive.read(index)
        manifest = json_object(manifest_raw)
        if manifest["version"] != 1 or manifest["scope"] != "packaging_only":
            raise ValueError("Unsupported component manifest")
        entries = {row["path"]: row for row in manifest["files"]}
        if len(entries) != len(manifest["files"]) or entries.keys() != infos.keys():
            raise ValueError("Component files differ from the manifest")
        if entries["runtime/OpenRGB.exe"]["sha256"] != manifest["engine_sha256"]:
            raise ValueError("OpenRGB executable pin differs from the component manifest")
        if not re.fullmatch(r"[0-9a-f]{40}", manifest["engine_revision"]):
            raise ValueError("Expected a full OpenRGB source revision")
        with archive.open(infos["runtime/OpenRGB.exe"]) as image:
            check_amd64_executable(image, "OpenRGB.exe", runtime_floor)
        destination.mkdir(parents=True)
        for name, row in sorted(entries.items()):
            path = destination.joinpath(*safe_archive_name(name).parts)
            if row["size"] != infos[name].file_size:
                raise ValueError(f"Component file size mismatch: {name}")
            path.parent.mkdir(parents=True, exist_ok=True)
            with archive.open(infos[name]) as reader, path.open("xb") as writer:
                shutil.copyfileobj(reader, writer)
            file_pin(path, row["sha256"])
        (destination / "component-manifest.json").write_bytes(manifest_raw)
    return {
        "relative_executable": BACKEND,
        "sha256": manifest["engine_sha256"],
        "revision": manifest["engine_revision"],
        "component_archive_sha256": expected,
    }
