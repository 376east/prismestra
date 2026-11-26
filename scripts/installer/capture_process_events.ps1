[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$OutputPath,
    [ValidateRange(1, 7200)][int]$DurationSeconds = 900,
    [Parameter(Mandatory)][switch]$DisposableGuest
)

$ErrorActionPreference = 'Stop'
if (-not $DisposableGuest) {
    throw 'Only run this process capture in a disposable Windows guest.'
}
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
try {
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'Run this process event capture from an elevated guest session.'
    }
} finally {
    $identity.Dispose()
}

$target = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutputPath)
if ((Test-Path -LiteralPath $target) -or
    -not [IO.Directory]::Exists([IO.Path]::GetDirectoryName($target))) {
    throw 'The output file must be new and its parent directory must exist.'
}

$prefix = 'Prismestra.ProcessTrace.' + [Guid]::NewGuid().ToString('N')
$sources = [ordered]@{
    start = ${prefix} + '.start'
    stop = ${prefix} + '.stop'
}
$subscribed = [Collections.Generic.List[string]]::new()
$events = [Collections.Generic.List[object]]::new()

function Receive-ProcessEvents {
    foreach ($kind in $sources.Keys) {
        foreach ($queued in (Get-Event -SourceIdentifier $sources[$kind] -ErrorAction SilentlyContinue)) {
            $event = $queued.SourceEventArgs.NewEvent
            if ($null -eq $event -or $null -eq $event.TIME_CREATED -or $null -eq $event.ProcessID) {
                throw 'The process event source returned an incomplete event.'
            }
            if ($events.Count -ge 50000) {
                throw 'Process capture exceeded 50,000 events. Use a cleaner guest or a shorter interval.'
            }
            $eventTime = [uint64]$event.TIME_CREATED
            $createdTime = $null
            if ($kind -eq 'start' -and [uint32]$event.ProcessID -ne 0) {
                $live = Get-Process -Id $event.ProcessID -ErrorAction SilentlyContinue
                if ($null -ne $live) {
                    try {
                        $name = [IO.Path]::GetFileNameWithoutExtension([string]$event.ProcessName)
                        $created = $live.StartTime.ToUniversalTime().ToFileTimeUtc()
                        if ($live.ProcessName -ieq $name -and $live.SessionId -eq $event.SessionID -and
                            $created -gt 0 -and $created -le $eventTime) {
                            $createdTime = $created.ToString([Globalization.CultureInfo]::InvariantCulture)
                        }
                    } catch {
                        # The process can exit before CIM delivers the event.
                    } finally {
                        $live.Dispose()
                    }
                }
            }
            $events.Add([ordered]@{
                kind = $kind
                filetime = $eventTime.ToString([Globalization.CultureInfo]::InvariantCulture)
                created_filetime = $createdTime
                pid = [uint32]$event.ProcessID
                parent_pid = [uint32]$event.ParentProcessID
                session_id = [uint32]$event.SessionID
                image = [string]$event.ProcessName
            })
            Remove-Event -EventIdentifier $queued.EventIdentifier
        }
    }
}

function Invoke-TraceControl {
    $before = [DateTime]::UtcNow.ToFileTimeUtc()
    # This deliberate short-lived shell runs outside the measured product interval.
    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = Join-Path ([Environment]::GetFolderPath([Environment+SpecialFolder]::System)) 'cmd.exe'
    $startInfo.Arguments = '/d /c exit 0'
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $control = [Diagnostics.Process]::Start($startInfo)
    try {
        $controlPid = $control.Id
        if (-not $control.WaitForExit(5000) -or $control.ExitCode -ne 0) {
            throw 'The process capture control did not exit successfully.'
        }
    } finally {
        $control.Dispose()
    }
    $wait = [Diagnostics.Stopwatch]::StartNew()
    do {
        Receive-ProcessEvents
        $start = @($events | Where-Object {
            $_.kind -eq 'start' -and $_.pid -eq $controlPid -and $_.parent_pid -eq $PID -and
            $_.image -ieq 'cmd.exe' -and [uint64]$_.filetime -ge $before
        })
        if ($start.Count -eq 1) {
            $stop = @($events | Where-Object {
                $_.kind -eq 'stop' -and $_.pid -eq $controlPid -and $_.image -ieq 'cmd.exe' -and
                [uint64]$_.filetime -ge [uint64]$start[0].filetime
            })
            if ($stop.Count -eq 1) {
                return [ordered]@{ pid = $controlPid; start_filetime = $start[0].filetime; stop_filetime = $stop[0].filetime }
            }
        }
        Start-Sleep -Milliseconds 100
    } while ($wait.Elapsed.TotalSeconds -lt 10)
    throw 'The capture control did not produce both a process start and a process stop event.'
}

try {
    $null = Register-CimIndicationEvent -Namespace root/cimv2 -ClassName Win32_ProcessStartTrace -SourceIdentifier $sources.start
    $subscribed.Add($sources.start)
    $null = Register-CimIndicationEvent -Namespace root/cimv2 -ClassName Win32_ProcessStopTrace -SourceIdentifier $sources.stop
    $subscribed.Add($sources.stop)

    $before = Invoke-TraceControl
    $started = [DateTime]::UtcNow.ToFileTimeUtc()
    Write-Host "Process capture ready for $DurationSeconds seconds. Start the intended scenario in the guest."
    $elapsed = [Diagnostics.Stopwatch]::StartNew()
    do {
        Receive-ProcessEvents
        Start-Sleep -Milliseconds 100
    } while ($elapsed.Elapsed.TotalSeconds -lt $DurationSeconds)
    $ended = [DateTime]::UtcNow.ToFileTimeUtc()
    $after = Invoke-TraceControl
    Receive-ProcessEvents

    $receipt = [ordered]@{
        format = 'prismestra-windows-process-events-v1'
        started_filetime = $started.ToString([Globalization.CultureInfo]::InvariantCulture)
        ended_filetime = $ended.ToString([Globalization.CultureInfo]::InvariantCulture)
        harness_pid = $PID
        before_control = $before
        after_control = $after
        events = $events.ToArray()
    }
    $bytes = [Text.UTF8Encoding]::new($false).GetBytes(($receipt | ConvertTo-Json -Depth 6))
    $file = [IO.File]::Open($target, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
    try {
        $file.Write($bytes, 0, $bytes.Length)
    } finally {
        $file.Dispose()
    }
    Write-Host "Saved $($events.Count) process events. Window and scenario coverage require separate evidence."
} finally {
    foreach ($subscription in $subscribed) {
        Unregister-Event -SourceIdentifier $subscription -ErrorAction SilentlyContinue
        Get-Event -SourceIdentifier $subscription -ErrorAction SilentlyContinue |
            Remove-Event -ErrorAction SilentlyContinue
    }
}
