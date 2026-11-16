# Dot-source from a Windows helper launcher. No tasks, processes or settings are changed.
# Must match app/backend_helper Request v2; this is NOT a UTC timestamp.
function Get-PrismestraActiveMilliseconds {
    if (-not ('Prismestra.ActiveClock' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
namespace Prismestra {
    public static class ActiveClock {
        [DllImport("kernel32.dll", SetLastError=true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        static extern bool QueryUnbiasedInterruptTime(out ulong ticks);
        public static ulong Milliseconds() {
            ulong ticks;
            if (!QueryUnbiasedInterruptTime(out ticks)) throw new Win32Exception(Marshal.GetLastWin32Error());
            return ticks / 10000UL;
        }
    }
}
'@
    }
    [Prismestra.ActiveClock]::Milliseconds()
}

function Test-PrismestraOwnerClock($Request, [UInt64]$NowMs) {
    if (($Request.version -isnot [int] -and $Request.version -isnot [long]) -or
        $Request.version -ne 2 -or
        $Request.clock_domain -cne 'windows-unbiased-interrupt-ms-v1' -or
        ($Request.timestamp_ms -isnot [int] -and $Request.timestamp_ms -isnot [long] -and $Request.timestamp_ms -isnot [UInt64]) -or
        $Request.timestamp_ms -lt 0) { return $false }
    $stamp = [UInt64]$Request.timestamp_ms
    $stamp -le $NowMs -and ($NowMs - $stamp) -le 5000
}
