"""Stage hash-pinned inputs and build Prismestra-Setup-<version>-x64.exe."""

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import stat
import struct
import subprocess
import sys
import tempfile
from urllib.parse import urlsplit
import zipfile

from package_release import sha256
from components.verify_openrgb_component import pe_imports


ROOT = Path(__file__).resolve().parent
BACKEND = "components/openrgb/runtime/OpenRGB.exe"
SHA256 = re.compile(r"[0-9a-f]{64}")
APP_ID = "9bb22971-3e88-434d-b212-ab8cf717799a"
WEBVIEW_PROBE = "Prismestra-Setup-Probe.exe"
WEBVIEW_PROBE_IMPORTS = {
    "advapi32.dll", "api-ms-win-core-synch-l1-2-0.dll", "kernel32.dll", "ntdll.dll",
}
POLICY_TOOL = "Prismestra-Setup-Policy.exe"
LEGACY_STARTUP_TOOL = "Prismestra-Legacy-Startup.exe"
POLICY_TOOL_IMPORTS = WEBVIEW_PROBE_IMPORTS | {
    "combase.dll", "ole32.dll", "oleaut32.dll", "shell32.dll",
}
LEGACY_STARTUP_TOOL_IMPORTS = POLICY_TOOL_IMPORTS | {"bcryptprimitives.dll", "user32.dll"}
PREREQUISITES = {
    "vc_redist_x64": ("vc_redist.x64.exe", 10 << 20, "Microsoft Corporation", {"aka.ms", "download.visualstudio.microsoft.com"}),
    "webview2_x64": ("MicrosoftEdgeWebView2RuntimeInstallerX64.exe", 30 << 20, "Microsoft Corporation", {"go.microsoft.com", "msedge.sf.dl.delivery.mp.microsoft.com", "msedge.api.cdp.microsoft.com"}),
    "pawnio": ("PawnIO_setup.exe", 1 << 16, "namazso.eu", {"pawnio.eu", "github.com"}),
}
MAX_COMPONENT_BYTES = 1 << 30


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


def write_json(path, data):
    path.write_bytes((json.dumps(data, sort_keys=True, indent=2, ensure_ascii=False) + "\n").encode())


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


def authenticode(path, publisher):
    if sys.platform != "win32":
        raise ValueError("Prerequisite Authenticode verification requires Windows")
    env = dict(os.environ, PRISMESTRA_PACKAGE_SIGNATURE_FILE=str(path))
    powershell = Path(os.environ["SystemRoot"]) / "System32/WindowsPowerShell/v1.0/powershell.exe"
    env["PSMODULEPATH"] = str(powershell.parent / "Modules")
    result = subprocess.run(
        [str(powershell), "-NoProfile", "-NonInteractive", "-Command",
         "$s=Get-AuthenticodeSignature -LiteralPath $env:PRISMESTRA_PACKAGE_SIGNATURE_FILE; "
         "if ($s.Status -ne 'Valid') { exit 1 }; "
         "$s.SignerCertificate.GetNameInfo([System.Security.Cryptography.X509Certificates.X509NameType]::SimpleName,$false)"],
        env=env, text=True, capture_output=True,
    )
    if result.returncode:
        raise ValueError(f"Authenticode validation failed for {path.name}: {result.stderr.strip()}")
    signature = result.stdout.strip()
    if signature.casefold() != publisher.casefold():
        raise ValueError(f"Unexpected prerequisite publisher for {path.name}: {signature}")


def read_prerequisite_lock(lock_path):
    lock = json_object(lock_path.read_bytes())
    packages = lock["packages"]
    if (lock["version"] != 1 or lock["architecture"] != "x64" or not isinstance(packages, dict)
            or not {"vc_redist_x64", "webview2_x64"} <= packages.keys()
            or not packages.keys() <= PREREQUISITES.keys()):
        raise ValueError("The prerequisite lock must pin x64 VC and WebView2 standalone installers")
    floor = parse_vc_version(lock.get("minimum_vc_runtime"), "minimum_vc_runtime")
    vc = packages["vc_redist_x64"]
    bundled = parse_vc_version(vc.get("version") if isinstance(vc, dict) else None, "bundled VC installer version")
    if floor > bundled:
        raise ValueError("minimum_vc_runtime exceeds the bundled VC installer version")
    return packages, floor


def parse_vc_version(value, label):
    if not isinstance(value, str) or not re.fullmatch(r"14(?:\.[0-9]{1,5}){3}", value):
        raise ValueError(f"{label} must be a four-part VC 14 runtime version")
    parts = tuple(map(int, value.split(".")))
    if any(part > 65535 for part in parts):
        raise ValueError(f"{label} contains a component above 65535")
    return parts


def stage_prerequisites(packages, source, destination):
    destination.mkdir()
    for key, package in sorted(packages.items()):
        filename, min_size, publisher, hosts = PREREQUISITES[key]
        if set(package) != {"file", "size", "sha256", "publisher", "source", "version"}:
            raise ValueError(f"Unexpected prerequisite lock fields for {key}")
        url = urlsplit(package["source"])
        if (package["file"] != filename
                or package["publisher"].casefold() != publisher.casefold()
                or url.scheme != "https" or url.netloc not in hosts
                or (key == "pawnio" and url.netloc == "github.com"
                    and not url.path.startswith("/namazso/PawnIO.Setup/releases/download/"))
                or not re.fullmatch(r"\d+\.\d+\.\d+(?:\.\d+)?", package["version"])
                or type(package["size"]) is not int or not min_size <= package["size"] <= 512 << 20):
            raise ValueError(f"Invalid offline prerequisite pin: {key}")
        pinned = source / filename
        file_pin(pinned, package["sha256"])
        if pinned.stat().st_size != package["size"]:
            raise ValueError(f"Prerequisite size mismatch: {filename}")
        staged = destination / filename
        shutil.copyfile(pinned, staged)
        file_pin(staged, package["sha256"])
        authenticode(staged, publisher)


def stage_app_notices(release_dir, payload, app_sha, commit):
    release = json_object((release_dir / "release-manifest.json").read_bytes())
    if (release["product"], release["commit"], release["executable_sha256"]) != ("Prismestra", commit, app_sha):
        raise ValueError("Application notices do not belong to the pinned release")
    source_name = safe_archive_name(release["source_archive"])
    if len(source_name.parts) != 1:
        raise ValueError("Expected a source archive filename in the release manifest")
    license_path = release_dir / "LICENSE"
    if license_path.is_symlink() or not license_path.is_file():
        raise ValueError("Missing application LICENSE")
    shutil.copyfile(license_path, payload / "LICENSE")
    notices = release_dir / "licenses"
    if notices.is_symlink() or not notices.is_dir():
        raise ValueError("Missing application license notices")
    for source in sorted(notices.rglob("*")):
        if source.is_symlink() or not (source.is_file() or source.is_dir()):
            raise ValueError("Unsafe release notice path")
        if source.is_file():
            target = payload / "licenses" / source.relative_to(notices)
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, target)
    if not (payload / "licenses").is_dir():
        raise ValueError("Empty release license notices")
    return source_name.as_posix()


def stage_webview_probe(destination):
    probe = destination / WEBVIEW_PROBE
    subprocess.run([
        "rustc", "--edition", "2024", "--target", "x86_64-pc-windows-msvc",
        "-C", "target-feature=+crt-static", "-C", "opt-level=s",
        "--crate-name", "prismestra_setup_probe", "-o", str(probe),
        str(ROOT / "installer/webview-probe.rs"),
    ], check=True)
    with probe.open("rb") as image:
        check_amd64_executable(image, WEBVIEW_PROBE, require_gui=True)
    imports = pe_imports(probe.read_bytes())
    if not imports["normal"] or imports["delay"] or set(imports["normal"]) - WEBVIEW_PROBE_IMPORTS:
        raise ValueError(f"Setup WebView2 probe must use only inbox Windows libraries: {imports}")
    return sha256(probe)


def stage_setup_tools(destination):
    target = "x86_64-pc-windows-msvc"
    hashes = {}
    with tempfile.TemporaryDirectory(prefix="prismestra-setup-tools-") as build:
        subprocess.run([
            "cargo", "build", "--locked", "--release", "--bins", "--target", target,
            "--manifest-path", str(ROOT / "installer/policy-tool/Cargo.toml"),
            "--target-dir", build,
        ], check=True, env={**os.environ, "CARGO_ENCODED_RUSTFLAGS": "-C\x1ftarget-feature=+crt-static"})
        for binary, installed_name, allowed_imports in (
            ("prismestra-setup-policy.exe", POLICY_TOOL, POLICY_TOOL_IMPORTS),
            ("prismestra-legacy-startup.exe", LEGACY_STARTUP_TOOL, LEGACY_STARTUP_TOOL_IMPORTS),
        ):
            source = Path(build) / target / "release" / binary
            with source.open("rb") as image:
                check_amd64_executable(image, installed_name, require_gui=True)
            imports = pe_imports(source.read_bytes())
            if not imports["normal"] or imports["delay"] or set(imports["normal"]) - allowed_imports:
                raise ValueError(f"{installed_name} must use only inbox Windows libraries: {imports}")
            tool = destination / installed_name
            shutil.copyfile(source, tool)
            hashes[installed_name] = sha256(tool)
    return hashes


def package(args):
    if not re.fullmatch(r"[0-9a-f]{40}", args.expected_commit):
        raise ValueError("Use a full lowercase Git commit SHA")
    if not re.fullmatch(r"\d{1,5}\.\d{1,5}\.\d{1,5}(?:\.\d{1,5})?", args.version):
        raise ValueError("Use a numeric installer version, such as 0.1.0.5")
    if any(int(part) > 65535 for part in args.version.split(".")):
        raise ValueError("Installer version components must fit in a 16-bit number")
    test_mode = args.test_namespace is not None
    if test_mode:
        if not re.fullmatch(r"[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}", args.test_namespace):
            raise ValueError("Use a lowercase GUID for the isolated test namespace")
        if args.test_namespace == APP_ID:
            raise ValueError("Test namespace must differ from the production AppId")
        if args.prerequisite_lock or args.prerequisite_dir:
            raise ValueError("Lifecycle fixtures cannot contain prerequisites or drivers")
    elif not all((args.prerequisite_lock, args.prerequisite_dir, args.release_dir)):
        raise ValueError("Production requires the offline prerequisite lock, its files and the release notices")
    if not args.stage_only and not args.iscc:
        raise ValueError("Provide --iscc or choose --stage-only")
    packages, runtime_floor = ({}, None) if test_mode else read_prerequisite_lock(args.prerequisite_lock)
    file_pin(args.app_exe, args.app_sha256)
    with args.app_exe.open("rb") as image:
        check_amd64_executable(image, "Prismestra.exe", runtime_floor)

    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    stage = output / "stage"
    payload = stage / "payload"
    payload.mkdir(parents=True)
    backend = extract_component(
        args.component_zip, payload / "components/openrgb", args.component_sha256, runtime_floor
    )
    app = payload / "Prismestra.exe"
    shutil.copyfile(args.app_exe, app)
    file_pin(app, args.app_sha256)
    release_source = stage_app_notices(args.release_dir, payload, args.app_sha256, args.expected_commit) if args.release_dir else None
    webview_probe_hash = None
    if not test_mode:
        stage_prerequisites(packages, args.prerequisite_dir, stage / "prerequisites")
        webview_probe_hash = stage_webview_probe(stage / "prerequisites")
        setup_tool_hashes = stage_setup_tools(payload)
    manifest = {
        "version": 1, "product": "Prismestra", "commit": args.expected_commit,
        "app_sha256": args.app_sha256, "app_release_source": release_source,
        "backend": backend, "prerequisites": packages,
        "installer_version": args.version, "data_namespace": args.test_namespace,
    }
    if runtime_floor is not None:
        manifest["minimum_vc_runtime"] = ".".join(map(str, runtime_floor))
    write_json(payload / "install-manifest.json", manifest)
    app_name = "Prismestra Lifecycle Test" if test_mode else "Prismestra"
    app_dir = f"{app_name}\\{args.test_namespace}" if test_mode else app_name
    if test_mode:
        stem = f"Prismestra-{args.version}-{args.expected_commit[:12]}-x64-test-{args.test_namespace}"
    else:
        stem = f"Prismestra-Setup-{args.version}-x64"
    compiler_defs = [
        ("ProductName", app_name), ("ProductDirectory", app_dir),
        ("InstallerVersion", args.version), ("InstallerBaseName", stem),
        ("InstallerAppId", args.test_namespace or APP_ID),
        ("ShortcutName", f"{app_name} {args.test_namespace}" if test_mode else app_name),
        ("StartupValue", f"Prismestra-Test-{args.test_namespace}" if test_mode else "Prismestra"),
    ]
    defs = [f'#define {name} "{value}"' for name, value in compiler_defs]
    if test_mode:
        defs.append("#define LifecycleTest")
    else:
        defs.append(f'#define WebViewProbeSha256 "{webview_probe_hash}"')
        defs.append(f'#define PolicyToolSha256 "{setup_tool_hashes[POLICY_TOOL]}"')
        defs.append(f'#define LegacyStartupToolSha256 "{setup_tool_hashes[LEGACY_STARTUP_TOOL]}"')
    if "pawnio" in packages:
        defs.append("#define IncludePawnIO")
    if runtime_floor is not None:
        for name, part in zip(("VcMajor", "VcMinor", "VcBuild", "VcRevision"), runtime_floor):
            defs.append(f"#define {name} {part}")
    (stage / "generated.iss").write_bytes(("\n".join(defs) + "\n").encode("utf-8"))
    for name in ("Prismestra.iss", "guard-native.iss"):
        shutil.copyfile(ROOT / "installer" / name, stage / name)

    receipt = {"scope": "staged_only", "manifest_sha256": sha256(payload / "install-manifest.json")}
    if webview_probe_hash is not None:
        receipt["webview_probe_sha256"] = webview_probe_hash
    if not args.stage_only:
        iscc = args.iscc.resolve(strict=True)
        subprocess.run([str(iscc), "/Qp", str(stage / "Prismestra.iss")], cwd=stage, check=True)
        installer = stage / "dist" / (stem + ".exe")
        if not installer.is_file():
            raise ValueError("Inno Setup did not produce the expected single installer")
        receipt.update(scope="compiled_only", installer=installer.relative_to(output).as_posix(),
                       installer_sha256=sha256(installer), installer_bytes=installer.stat().st_size)
    write_json(output / "installer-receipt.json", receipt)
    return receipt


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("app-exe", "component-zip", "output"):
        parser.add_argument("--" + name, required=True, type=Path)
    for name in ("app-sha256", "component-sha256", "expected-commit", "version"):
        parser.add_argument("--" + name, required=True)
    for name in ("release-dir", "prerequisite-lock", "prerequisite-dir", "iscc"):
        parser.add_argument("--" + name, type=Path)
    parser.add_argument("--test-namespace", "--data-namespace", dest="test_namespace")
    parser.add_argument("--stage-only", action="store_true")
    args = parser.parse_args()
    try:
        print(json.dumps(package(args), sort_keys=True))
    except (KeyError, OSError, TypeError, ValueError, subprocess.CalledProcessError, zipfile.BadZipFile) as error:
        parser.exit(1, f"Installer packaging: {error}\n")


if __name__ == "__main__":
    main()
