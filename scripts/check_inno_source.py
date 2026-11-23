"""Check the real production Inno script with ISCC output disabled."""

import argparse
from pathlib import Path
import subprocess
from tempfile import TemporaryDirectory
import time

from build_windows_candidate import INNO_INPUT, INSTALLER, install_inno
from package_installer import (
    LEGACY_STARTUP_TOOL, POLICY_TOOL, WEBVIEW_PROBE, read_prerequisite_lock, stage_inno_source,
)
from pinned_setup_download import fetch


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--iscc", type=Path, help="Reuse a local copy of the pinned compiler")
    args = parser.parse_args()
    with TemporaryDirectory(prefix="prismestra-inno-check-") as directory:
        root = Path(directory)
        if args.iscc is None:
            started = time.monotonic()
            bootstrap = fetch(root, "inno-bootstrap.exe", *INNO_INPUT)
            iscc = install_inno(root, bootstrap)
            print(f"Pinned ISCC ready in {time.monotonic() - started:.2f} s", flush=True)
        else:
            iscc = args.iscc.resolve(strict=True)

        stage = root / "source"
        prereqs = stage / "prerequisites"
        prereqs.mkdir(parents=True)
        packages, runtime_floor = read_prerequisite_lock(INSTALLER / "prerequisites.lock.json")
        # /O- still checks [Files] paths. Empty placeholders suffice for syntax checking.
        for name in (WEBVIEW_PROBE, *(pin["file"] for pin in packages.values())):
            (prereqs / name).touch()
        payload = stage / "payload"
        payload.mkdir()
        (payload / "placeholder.txt").touch()
        fake_hash = "0" * 64
        stage_inno_source(
            stage, "0.0.0.1", "SyntaxCheck", test_namespace=None, webview_probe_hash=fake_hash,
            setup_tool_hashes={POLICY_TOOL: fake_hash, LEGACY_STARTUP_TOOL: fake_hash},
            include_pawnio="pawnio" in packages, runtime_floor=runtime_floor,
        )
        started = time.monotonic()
        subprocess.run([iscc, "/O-", "/Qp", stage / "Prismestra.iss"], cwd=stage, check=True)
        print(f"Inno source check passed in {time.monotonic() - started:.2f} s", flush=True)


if __name__ == "__main__":
    main()
