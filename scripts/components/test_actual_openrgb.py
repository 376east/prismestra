"""Opt-in Windows SDK smoke against an owned, hash-pinned --nodetect server.

No hardware discovery, mode changes, administrative setup or installed-app state.
Artifacts are preserved in a newly created directory supplied by the caller.
"""
import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import socket
import struct
import subprocess
import time

SDK_TEST = "transport::openrgb::tests::candidate_headless_sdk6_client_smoke"


def listener_owned(port, pid):
    size = ctypes.c_ulong(0)
    table = ctypes.windll.iphlpapi.GetExtendedTcpTable
    result = table(None, ctypes.byref(size), False, socket.AF_INET, 3, 0)
    if result != 122 or size.value < 4 or size.value > 16 * 1024 * 1024:
        return False
    buffer = ctypes.create_string_buffer(size.value)
    if table(buffer, ctypes.byref(size), False, socket.AF_INET, 3, 0):
        return False
    data = buffer.raw[:size.value]
    count, = struct.unpack_from("<I", data)
    if 4 + count * 24 > len(data):
        return False
    for index in range(count):
        state, address, local_port, _, _, owner = struct.unpack_from("<6I", data, 4 + index * 24)
        if (state == 2 and owner == pid and socket.ntohs(local_port & 0xFFFF) == port
                and struct.pack("<I", address) == socket.inet_aton("127.0.0.1")):
            return True
    return False


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--exe", required=True, type=Path)
    parser.add_argument("--sha256", required=True)
    parser.add_argument("--client-test-exe", required=True, type=Path,
                        help="Prebuilt Rust library test executable; build it before this harness")
    parser.add_argument("--client-test-sha256", required=True)
    parser.add_argument("--artifacts", required=True, type=Path)
    args = parser.parse_args()
    if os.name != "nt":
        parser.error("Windows owner-table verification required")
    executable = args.exe.resolve(strict=True)
    actual = hashlib.sha256(executable.read_bytes()).hexdigest()
    if actual != args.sha256.lower():
        parser.error("executable SHA256 mismatch")
    client_test = args.client_test_exe.resolve(strict=True)
    client_digest = hashlib.sha256(client_test.read_bytes()).hexdigest()
    if client_digest != args.client_test_sha256.lower():
        parser.error("prebuilt client test SHA256 mismatch")
    artifacts = args.artifacts.absolute()
    artifacts.mkdir(parents=True, exist_ok=False)
    config = artifacts / "isolated-config"
    config.mkdir()
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    command = [str(executable), "--server", "--server-host", "127.0.0.1",
               "--server-port", str(port), "--noautoconnect", "--nodetect",
               "--config", str(config)]
    report = {"ok": False, "exe": str(executable), "sha256": actual,
              "client_test_exe": str(client_test), "client_test_sha256": client_digest,
              "command": command, "scope": "actual_sdk_no_hardware",
              "unverified": ["physical inventory", "Direct frames", "DeviceListUpdated",
                             "SMBus contention", "hardware recovery", "clean Windows"]}
    child = None
    try:
        environment = os.environ.copy()
        # ResourceManager/LogManager can read their default directory before
        # CLI --config is parsed. Keep even that early access inside our fixture.
        environment["APPDATA"] = str(config)
        environment["XDG_CONFIG_HOME"] = str(config)
        with (artifacts / "server.stdout.log").open("wb") as stdout, (
                artifacts / "server.stderr.log").open("wb") as stderr:
            child = subprocess.Popen(command, cwd=executable.parent, env=environment, stdout=stdout, stderr=stderr,
                                     creationflags=subprocess.CREATE_NO_WINDOW)
            report["pid"] = child.pid
            end = time.monotonic() + 15
            while not listener_owned(port, child.pid):
                if child.poll() is not None:
                    raise RuntimeError(f"owned server exited: {child.returncode}")
                if time.monotonic() >= end:
                    raise RuntimeError("owned loopback listener did not become ready")
                time.sleep(0.1)
            environment["PRISMESTRA_SDK_TEST_ENDPOINT"] = f"127.0.0.1:{port}"
            environment["CARGO_TERM_COLOR"] = "never"
            with (artifacts / "client.log").open("wb") as log:
                result = subprocess.run(
                    [str(client_test), SDK_TEST, "--ignored", "--exact", "--nocapture", "--color", "never"],
                    cwd=artifacts, env=environment,
                    stdout=log, stderr=subprocess.STDOUT, timeout=180, check=False)
            report["client_exit"] = result.returncode
            receipt = (artifacts / "client.log").read_text(encoding="utf-8", errors="replace")
            report["named_test_passed"] = (
                f"test {SDK_TEST} ... ok" in receipt
                and "test result: ok. 1 passed; 0 failed; 0 ignored;" in receipt
            )
            if (result.returncode or not report["named_test_passed"]
                    or child.poll() is not None or not listener_owned(port, child.pid)):
                raise RuntimeError("client test failed or server ownership was lost; inspect artifacts")
            report["ok"] = True
    except (OSError, RuntimeError, subprocess.SubprocessError) as error:
        report["error"] = str(error)
    finally:
        if child is not None:
            if child.poll() is None:
                child.terminate()
            try:
                child.wait(timeout=10)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait(timeout=10)
            report["owned_child_stopped"] = child.poll() is not None
        (artifacts / "result.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2))
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
