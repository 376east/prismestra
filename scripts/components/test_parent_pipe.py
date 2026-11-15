"""Actual Windows component lifetime-pipe checks, with hardware detection disabled."""
import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import socket
import subprocess
import time

from test_actual_openrgb import listener_owned


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--exe', type=Path, required=True)
    parser.add_argument('--sha256', required=True)
    parser.add_argument('--artifacts', type=Path, required=True)
    args = parser.parse_args()
    args.artifacts = args.artifacts.resolve()
    exe = args.exe.resolve(strict=True)
    if os.name != 'nt' or hashlib.sha256(exe.read_bytes()).hexdigest() != args.sha256.lower():
        parser.error('Windows and an exact executable SHA256 are required')
    args.artifacts.mkdir(parents=True, exist_ok=False)
    report = {'ok': False, 'sha256': args.sha256, 'scope': 'actual component; no hardware discovery',
              'unverified': ['hardware cleanup', 'kernel IOCTL hang', 'owner hard crash'], 'cases': []}
    ctypes.windll.kernel32.GetConsoleWindow.restype = ctypes.c_void_p
    report['parent_console_present'] = bool(ctypes.windll.kernel32.GetConsoleWindow())
    try:
        for name, enabled, pipe, early in [
                ('eof_after_ready', True, True, False),
                ('eof_before_ready', True, True, True),
                ('standalone_pipe_unchanged', False, True, False),
                ('invalid_input_not_armed', True, False, False)]:
            directory = args.artifacts / name
            directory.mkdir()
            environment = os.environ.copy()
            environment['APPDATA'] = str(directory)
            environment.pop('PRISMESTRA_PARENT_PIPE', None)
            if enabled:
                environment['PRISMESTRA_PARENT_PIPE'] = '1'
            with socket.socket() as reservation:
                reservation.bind(('127.0.0.1', 0))
                port = reservation.getsockname()[1]
            command = [str(exe), '--server', '--server-host', '127.0.0.1', '--server-port', str(port),
                       '--noautoconnect', '--nodetect', '--config', str(directory)]
            case = {'name': name, 'ok': False}
            report['cases'].append(case)
            with (directory / 'stdout.log').open('wb') as stdout, (directory / 'stderr.log').open('wb') as stderr:
                child = subprocess.Popen(command, cwd=exe.parent, env=environment,
                                         stdin=subprocess.PIPE if pipe else subprocess.DEVNULL,
                                         stdout=stdout, stderr=stderr,
                                         creationflags=subprocess.CREATE_NO_WINDOW)
                case['pid'] = child.pid
                try:
                    if early:
                        child.stdin.close()
                    else:
                        deadline = time.monotonic() + 15
                        while not listener_owned(port, child.pid):
                            if child.poll() is not None or time.monotonic() >= deadline:
                                raise RuntimeError(f'{name}: server readiness failed')
                            time.sleep(0.05)
                        case['owned_listener_ready'] = True
                        if pipe:
                            child.stdin.close()
                    began = time.monotonic()
                    if enabled and pipe:
                        code = child.wait(timeout=3)
                        case.update(exit_code=code, exit_seconds=time.monotonic() - began)
                        if code != 0:
                            raise RuntimeError(f'{name}: nonzero cooperative exit')
                    else:
                        time.sleep(0.3)
                        if child.poll() is not None or not listener_owned(port, child.pid):
                            raise RuntimeError(f'{name}: standalone invocation exited unexpectedly')
                    case['ok'] = True
                finally:
                    if child.poll() is None:
                        child.kill()  # Only this retained --nodetect fixture child.
                    child.wait(timeout=10)
                    if child.stdin is not None and not child.stdin.closed:
                        child.stdin.close()
                    case['owned_child_stopped'] = child.poll() is not None
        report['ok'] = True
    except (OSError, RuntimeError, subprocess.SubprocessError) as error:
        report['error'] = str(error)
    finally:
        (args.artifacts / 'result.json').write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(report, indent=2))
    return 0 if report['ok'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
