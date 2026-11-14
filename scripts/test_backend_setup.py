"""Exercise backend setup in PowerShell 5 without running a backend or changing tasks."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).with_name('portable') / 'Configure OpenRGB.ps1'


@unittest.skipUnless(os.name == 'nt', 'Windows PowerShell required')
class BackendSetupChecks(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='ps-backend-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve() / "Путь [x] & O'Brien"
        self.root.mkdir()
        self.exe = self.root / 'OpenRGB.exe'
        self.exe.write_bytes(b'not executed: backend fixture')
        self.config = self.root / 'backend-config'
        self.config.mkdir()
        self.state = self.root / 'state'

    def setup_backend(self, *extra):
        return subprocess.run([shutil.which('powershell.exe'), '-NoProfile', '-ExecutionPolicy', 'Bypass',
                               '-File', str(SCRIPT.resolve()), '-Executable', str(self.exe),
                               '-ConfigDirectory', str(self.config), '-StateDirectory', str(self.state), *extra],
                              capture_output=True, text=True, timeout=15)

    def test_explicit_ipv4_and_ipv6_paths_pin_without_launching(self):
        for endpoint in ['127.0.0.1:6789', '[::1]:6790']:
            result = self.setup_backend('-Endpoint', endpoint, '-Force')
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            value = json.loads((self.state / 'managed-openrgb.json').read_text())
            self.assertEqual(value['endpoint'], endpoint)
            self.assertEqual(value['expected_executable_sha256'], hashlib.sha256(self.exe.read_bytes()).hexdigest())
            self.assertEqual(Path(value['executable']), self.exe)
            self.assertFalse((self.state / 'owner-request.json').exists())
            self.assertFalse((self.state / 'backend-task.json').exists())

    def test_remote_and_invalid_endpoints_leave_state_absent(self):
        for endpoint in ['192.0.2.1:6742', 'localhost:6742', '127.0.0.1:0', '127.0.0.1:99999']:
            result = self.setup_backend('-Endpoint', endpoint)
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(self.state.exists())

    def test_existing_configuration_requires_explicit_replace(self):
        first = self.setup_backend()
        self.assertEqual(first.returncode, 0, first.stdout + first.stderr)
        path = self.state / 'managed-openrgb.json'
        before = path.read_bytes()
        second = self.setup_backend('-Endpoint', '127.0.0.1:6791')
        self.assertNotEqual(second.returncode, 0)
        self.assertEqual(path.read_bytes(), before)


if __name__ == '__main__':
    unittest.main()
