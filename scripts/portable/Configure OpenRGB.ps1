# One-time local backend configuration. Does not install drivers or grant elevation.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Executable,
    [Parameter(Mandatory = $true)][string]$ConfigDirectory,
    [string]$StateDirectory = (Join-Path $PSScriptRoot 'data\state'),
    [string]$Endpoint = '127.0.0.1:6742',
    [string]$AuthorizedTaskName,
    [switch]$Force
)
$ErrorActionPreference = 'Stop'
try {
    $binary = [System.IO.Path]::GetFullPath($Executable)
    $config = [System.IO.Path]::GetFullPath($ConfigDirectory)
    $state = [System.IO.Path]::GetFullPath($StateDirectory)
    foreach ($path in @($binary, $config, $state)) {
        if ($path.StartsWith('\\')) { throw 'Backend configuration requires local disk paths.' }
    }
    if ($Endpoint -notmatch '^(\[[0-9a-fA-F:]+\]|[0-9.]+):([0-9]{1,5})$') {
        throw 'Use a numeric IP:port endpoint, with brackets around IPv6.'
    }
    $address = [System.Net.IPAddress]::Parse($Matches[1].Trim('[', ']'))
    $port = [int]$Matches[2]
    if (-not [System.Net.IPAddress]::IsLoopback($address) -or $port -lt 1 -or $port -gt 65535) {
        throw 'Managed OpenRGB must use an explicit loopback endpoint. Keep remote servers external.'
    }
    $socket = New-Object System.Net.IPEndPoint($address, $port)
    if (-not [System.IO.File]::Exists($binary)) { throw 'Select an existing OpenRGB executable.' }
    if (-not [System.IO.Directory]::Exists($config)) { throw 'Create an isolated OpenRGB configuration directory first.' }
    $destination = Join-Path $state 'managed-openrgb.json'
    $taskDestination = Join-Path $state 'backend-task.json'
    if (-not $Force -and ([System.IO.File]::Exists($destination) -or [System.IO.File]::Exists($taskDestination))) {
        throw 'Backend settings already exist. Back them up and use -Force to replace them.'
    }
    if ($AuthorizedTaskName) {
        & "$env:SystemRoot\System32\schtasks.exe" /Query /TN $AuthorizedTaskName 1>$null 2>$null
        if ($LASTEXITCODE -ne 0) {
            throw 'The authorized backend task does not exist. This command does not register elevated tasks.'
        }
    }
    [System.IO.Directory]::CreateDirectory($state) | Out-Null
    $algorithm = [System.Security.Cryptography.SHA256]::Create()
    $stream = [System.IO.File]::OpenRead($binary)
    try { $digest = [System.BitConverter]::ToString($algorithm.ComputeHash($stream)).Replace('-', '').ToLowerInvariant() }
    finally { $stream.Dispose(); $algorithm.Dispose() }
    $settings = [ordered]@{
        version = 1; enabled = $true; executable = $binary; config_dir = $config
        endpoint = $socket.ToString(); expected_executable_sha256 = $digest
    }
    $encoding = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($destination, ($settings | ConvertTo-Json), $encoding)
    if ($AuthorizedTaskName) {
        [System.IO.File]::WriteAllText($taskDestination, (@{version=1; task_name=$AuthorizedTaskName} | ConvertTo-Json), $encoding)
    } elseif ([System.IO.File]::Exists($taskDestination)) {
        [System.IO.File]::Delete($taskDestination)
    }
    Write-Output 'Local OpenRGB management is configured. Start Prismestra with your saved connections; no separate OpenRGB launch is needed.'
} catch {
    [Console]::Error.WriteLine('OpenRGB setup: ' + $_.Exception.Message)
    exit 1
}
