"""Check the Windows launcher/native clock contract without starting the application."""
import base64
import os
from pathlib import Path
import shutil
import subprocess
import unittest

SCRIPT = Path(__file__).with_name('windows') / 'Helper Clock.ps1'


@unittest.skipUnless(os.name == 'nt', 'Windows helper clock required')
class HelperClockChecks(unittest.TestCase):
    def test_native_active_clock_and_fail_closed_v2_schema(self):
        path = str(SCRIPT.resolve()).replace("'", "''")
        body = """
$ErrorActionPreference='Stop'
. 'SCRIPT_PATH'
$start=Get-PrismestraActiveMilliseconds
Start-Sleep -Milliseconds 20
if((Get-PrismestraActiveMilliseconds) -lt $start){throw 'Active clock regressed'}
$good='{"version":2,"clock_domain":"windows-unbiased-interrupt-ms-v1","timestamp_ms":10000}'|ConvertFrom-Json
if(-not(Test-PrismestraOwnerClock $good 15000)){throw 'Lease boundary rejected'}
if(Test-PrismestraOwnerClock $good 15001){throw 'Expired lease accepted'}
if(Test-PrismestraOwnerClock $good 9999){throw 'Future lease accepted'}
foreach($json in @(
 '{"version":1,"clock_domain":"windows-unbiased-interrupt-ms-v1","timestamp_ms":10000}',
 '{"version":2,"clock_domain":"utc","timestamp_ms":10000}',
 '{"version":2,"timestamp_ms":10000}',
 '{"version":true,"clock_domain":"windows-unbiased-interrupt-ms-v1","timestamp_ms":10000}',
 '{"version":2,"clock_domain":"windows-unbiased-interrupt-ms-v1","timestamp_ms":true}',
 '{"version":2,"clock_domain":"windows-unbiased-interrupt-ms-v1","timestamp_ms":10000.0}',
 '{"version":2,"clock_domain":"windows-unbiased-interrupt-ms-v1","timestamp_ms":-1}'
)) {if(Test-PrismestraOwnerClock ($json|ConvertFrom-Json) 10000){throw ('Invalid schema accepted: '+$json)}}
'PASS'
""".replace('SCRIPT_PATH', path)
        encoded = base64.b64encode(body.encode('utf-16le')).decode('ascii')
        shells = [shutil.which(name) for name in ('powershell.exe', 'pwsh.exe')]
        self.assertTrue(any(shells), 'A PowerShell host is required')
        for shell in filter(None, shells):
            with self.subTest(shell=shell):
                result = subprocess.run([shell, '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-EncodedCommand', encoded],
                                        capture_output=True, text=True, timeout=30)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn('PASS', result.stdout)


if __name__ == '__main__':
    unittest.main()
