# Package-root entry point. Compatible with Windows PowerShell 5.1.
[CmdletBinding()]
param(
    [switch]$Connected,
    [switch]$ClassicEditor,
    [switch]$Check
)

$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)

function Assert-StateAccess([string]$Path) {
    $directory = $Path
    while (-not [System.IO.Directory]::Exists($directory)) {
        if ([System.IO.File]::Exists($directory)) {
            throw "State path is a file: $directory. Choose a writable extraction folder."
        }
        $directory = [System.IO.Path]::GetDirectoryName($directory)
        if (-not $directory) { throw "Cannot find a parent directory for state: $Path" }
    }
    # Writability is probed only on launch; -Check must not create anything.
}

function ConvertTo-NativeArgument([string]$Value) {
    # Windows CommandLineToArgvW/CRT quoting, including trailing backslashes.
    '"' + [regex]::Replace([regex]::Replace($Value, '(\\*)"', '$1$1\"'), '(\\+)$', '$1$1') + '"'
}

function Wait-CoreStartup($Process, [string]$Path, [string]$Nonce) {
    $startupTimeoutMs = 15000
    $timer = [System.Diagnostics.Stopwatch]::StartNew()
    while ($timer.ElapsedMilliseconds -lt $startupTimeoutMs) {
        if ([System.IO.File]::Exists($Path)) {
            if ((Get-Item -LiteralPath $Path).Length -gt 16384) { throw 'Startup acknowledgement exceeds limit.' }
            try { $ack = [System.IO.File]::ReadAllText($Path) | ConvertFrom-Json } catch {
                throw 'Malformed startup acknowledgement.'
            }
            if ($ack.version -isnot [int] -or $ack.version -ne 1 -or
                $ack.pid -isnot [int] -or $ack.pid -ne $Process.Id -or
                $ack.nonce -isnot [string] -or $ack.nonce -cne $Nonce -or
                $ack.stage -isnot [string] -or
                $ack.detail -isnot [string] -or $ack.detail.Length -gt 4096) {
                throw 'Startup acknowledgement identity or schema mismatch.'
            }
            if ($ack.stage -ceq 'failed') { throw ("Native core startup failed: " + $ack.detail) }
            if ($ack.stage -cne 'core_ready') { throw 'Unknown startup acknowledgement stage.' }
            return
        }
        if ($Process.HasExited) {
            if ($Process.ExitCode -ne 0) { exit $Process.ExitCode }
            throw 'Prismestra exited before acknowledging native core startup.'
        }
        Start-Sleep -Milliseconds 25
    }
    throw 'Timed out waiting for native core startup acknowledgement. The spawned process was left running; inspect it before retrying.'
}

$root = [System.IO.Path]::GetFullPath($PSScriptRoot)
$exe = Join-Path $root 'Prismestra.exe'
$profile = Join-Path $root 'data\profile.json'
$state = Join-Path $root 'data\state'
$bindings = Join-Path $state 'lighting-local.json'
$plan = [ordered]@{
    ok = $false
    mode = $(if ($Connected) { 'connected' } else { 'preview' })
    executable = $exe
    working_directory = $root
    state_dir = $state
    profile = $null
    arguments = @()
    state_write_check = 'not tested by nonmutating Check; actual write probe on launch'
    error = $null
}

try {
    if (-not [System.IO.File]::Exists($exe)) {
        throw "Missing Prismestra.exe: $exe. Extract the complete release ZIP first."
    }
    if ([System.IO.Directory]::Exists($profile)) { throw "Profile path is a directory: $profile" }
    $hasProfile = [System.IO.File]::Exists($profile)
    if ($Connected -and -not $hasProfile) {
        throw "Missing saved profile: $profile. Open Prismestra in preview and save your profile there first."
    }
    if ($Connected -and -not [System.IO.File]::Exists($bindings)) {
        throw "Missing saved local connections: $bindings. Configure and save connections in Prismestra first."
    }
    Assert-StateAccess $state
    $nativeArguments = @('--state-dir', $state)
    if ($hasProfile) {
        $plan.profile = $profile
        $nativeArguments += @('--project', $profile)
    }
    if ($Connected) { $nativeArguments += '--start-connected' }
    if ($ClassicEditor) { $nativeArguments += '--classic-editor' }
    $plan.arguments = $nativeArguments
    $plan.ok = $true
    if ($Check) {
        $plan | ConvertTo-Json -Depth 4
        exit 0
    }
    $probe = Join-Path $state ('.launcher-write-' + [guid]::NewGuid().ToString('N'))
    try {
        [System.IO.Directory]::CreateDirectory($state) | Out-Null
        $stream = [System.IO.File]::Open($probe, 'CreateNew', 'Write', 'None')
        $stream.Dispose()
    } catch {
        throw "Cannot write portable state at $state. Extract the ZIP into a writable folder. $($_.Exception.Message)"
    } finally {
        if ([System.IO.File]::Exists($probe)) { [System.IO.File]::Delete($probe) }
    }
    $start = New-Object System.Diagnostics.ProcessStartInfo
    $start.FileName = $exe
    $start.WorkingDirectory = $root
    $start.UseShellExecute = $false
    $start.Arguments = ($nativeArguments | ForEach-Object { ConvertTo-NativeArgument $_ }) -join ' '
    $ackDirectory = Join-Path ([System.IO.Path]::GetTempPath()) ('prismestra-startup-' + [guid]::NewGuid().ToString('N'))
    [System.IO.Directory]::CreateDirectory($ackDirectory) | Out-Null
    $ackPath = Join-Path $ackDirectory 'ack.json'
    $nonce = [guid]::NewGuid().ToString('N')
    $start.EnvironmentVariables['PRISMESTRA_STARTUP_ACK_PATH'] = $ackPath
    $start.EnvironmentVariables['PRISMESTRA_STARTUP_ACK_NONCE'] = $nonce
    $process = $null
    try {
        $process = [System.Diagnostics.Process]::Start($start)
        Wait-CoreStartup $process $ackPath $nonce
        [Console]::WriteLine('Prismestra native core ready. Device/output readiness is reported by the application.')
    } finally {
        if ($null -ne $process) { $process.Dispose() }
        if ([System.IO.File]::Exists($ackPath)) { [System.IO.File]::Delete($ackPath) }
        # Only our known file and empty directory; never recursively delete child data.
        try { [System.IO.Directory]::Delete($ackDirectory) } catch { }
    }
    exit 0
} catch {
    if ($Check) {
        $plan.ok = $false
        $plan.error = $_.Exception.Message
        $plan | ConvertTo-Json -Depth 4
    } else {
        [Console]::Error.WriteLine("Prismestra launcher: " + $_.Exception.Message)
    }
    exit 1
}
