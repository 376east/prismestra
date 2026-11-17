"""Exercise the actual pinned lighting-only server without device discovery."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import socket
import struct
import subprocess
import time

from test_actual_openrgb import listener_owned


def packet(sock, command, data=b"", device=0, size=None):
    sock.sendall(struct.pack("<4sIII", b"ORGB", device, command, len(data) if size is None else size) + data)


def received(sock):
    def exact(n):
        result = b""
        while len(result) < n:
            block = sock.recv(n - len(result))
            if not block:
                raise EOFError("server closed")
            result += block
        return result
    magic, dev, command, size = struct.unpack("<4sIII", exact(16))
    if magic != b"ORGB" or size > 1048576:
        raise ValueError("unexpected response")
    return command, exact(size)


def reply(sock, expected):
    for _ in range(8):
        command, data = received(sock)
        if command == expected:
            return command, data
        if command != 10 or len(data) != 8 or struct.unpack_from("<I", data, 4)[0] != 0:
            raise AssertionError((command, data))
    raise AssertionError("response did not arrive")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--exe", required=True, type=Path)
    parser.add_argument("--sha256", required=True)
    parser.add_argument("--artifacts", required=True, type=Path)
    args = parser.parse_args()
    exe = args.exe.resolve(strict=True)
    if hashlib.sha256(exe.read_bytes()).hexdigest() != args.sha256:
        parser.error("binary pin mismatch")
    out = args.artifacts.absolute()
    out.mkdir(parents=True, exist_ok=False)
    config = out / "config"
    config.mkdir()
    with socket.socket() as reserve:
        reserve.bind(("127.0.0.1", 0))
        port = reserve.getsockname()[1]
    env = os.environ.copy()
    env.update(APPDATA=str(config), XDG_CONFIG_HOME=str(config))
    env.pop("PRISMESTRA_PARENT_PIPE", None)
    command = [str(exe), "--lighting-only", "--server", "--server-host", "127.0.0.1", "--server-port", str(port), "--noautoconnect", "--nodetect", "--config", str(config)]
    result = {"scope": "actual_lighting_sdk_without_hardware", "sha256": args.sha256, "cases": [], "unverified": ["physical Direct frames", "driver coexistence", "clean Windows"]}
    process = None
    try:
        with (out / "stdout.log").open("wb") as stdout, (out / "stderr.log").open("wb") as stderr:
            process = subprocess.Popen(command, cwd=exe.parent, env=env, stdout=stdout, stderr=stderr, creationflags=subprocess.CREATE_NO_WINDOW)
            deadline = time.monotonic() + 15
            while not listener_owned(port, process.pid):
                if process.poll() is not None or time.monotonic() >= deadline:
                    raise RuntimeError("owned server did not start")
                time.sleep(.05)
            with socket.create_connection(("127.0.0.1", port), timeout=2) as sock:
                packet(sock, 40, struct.pack("<I", 6))
                assert reply(sock, 40)[0] == 40
                assert reply(sock, 51)[0] == 51
                packet(sock, 52, struct.pack("<I", 1))
                response = reply(sock, 53)
                assert response == (53, struct.pack("<I", 1)), response
                packet(sock, 0)
                response = reply(sock, 0)
                assert response[0] == 0 and struct.unpack_from("<I", response[1])[0] == 0, response
            result["cases"].append("protocol6/RGB-only flags/empty inventory")
            cases = [(140, 0, b""), (150, 0, b""), (200, 0, b""), (250, 0, b""), (40000, 0, b""), (40001, 0, b""), (1100, 0, b""), (1050, 32775, b""), (1101, 32775, b""), (52, 4, struct.pack("<I", 65537)), (40, 4, struct.pack("<I", 5)), (1, 4, struct.pack("<I", 5))]
            for cmd, size, data in cases:
                with socket.create_connection(("127.0.0.1", port), timeout=2) as sock:
                    packet(sock, cmd, data, size=size)
                    try:
                        assert sock.recv(1) == b"", (cmd, "forbidden packet accepted")
                    except ConnectionResetError:
                        pass
                result["cases"].append(f"rejected {cmd}/{size}/{data.hex()}")
            assert process.poll() is None
            result["ok"] = True
    finally:
        if process is not None and process.poll() is None:
            process.terminate()
            process.wait(timeout=10)
        (out / "result.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result))


if __name__ == "__main__":
    main()
