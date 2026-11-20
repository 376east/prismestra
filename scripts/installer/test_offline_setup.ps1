[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Installer,
    [Parameter(Mandatory)][ValidatePattern('^[A-Fa-f0-9]{64}$')][string]$Sha256,
    [Parameter(Mandatory)][ValidatePattern('^[0-9a-f]{40}$')][string]$Commit,
    [Parameter(Mandatory)][ValidatePattern('^[0-9a-f]{64}$')][string]$ManifestSha256,
    [Parameter(Mandatory)][string]$ReportDirectory,
    [string]$PreviousInstaller,
    [switch]$ExerciseVcDownload,
    [switch]$DisposableRunner
)

$ErrorActionPreference = 'Stop'
if (-not $DisposableRunner -or $env:GITHUB_ACTIONS -cne 'true') {
    throw 'Use this installer test only on a disposable GitHub Actions runner.'
}
if ($ExerciseVcDownload -and ($env:RUNNER_ENVIRONMENT -cne 'github-hosted' -or $PreviousInstaller)) {
    throw 'VC Runtime download requires a dedicated GitHub-hosted fresh-install job.'
}
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]::new($identity)
if ($identity.IsSystem -or -not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator) -or
    -not [Environment]::Is64BitProcess -or [Runtime.InteropServices.RuntimeInformation]::OSArchitecture -ne 'X64') {
    throw 'Use an elevated x64 Windows PowerShell process under the disposable runner account.'
}

$appId = '9bb22971-3e88-434d-b212-ab8cf717799a'
$install = Join-Path ([Environment]::GetFolderPath('ProgramFiles')) 'Prismestra'
$image = Join-Path $install 'Prismestra.exe'
$userRoot = Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'Prismestra'
$manifestPath = Join-Path $install 'install-manifest.json'
$backend = 'components/openrgb/runtime/OpenRGB.exe'
$vcFilesRequired = @('vcruntime140.dll', 'vcruntime140_1.dll', 'msvcp140.dll', 'msvcp140_codecvt_ids.dll')
$knownAppLocalCrt = $vcFilesRequired + @('msvcp140_1.dll', 'msvcp140_2.dll', 'msvcp140_atomic_wait.dll', 'concrt140.dll')
$shortcut = Join-Path ([Environment]::GetFolderPath('CommonPrograms')) 'Prismestra.lnk'
$arpKey = "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\{${appId}}_is1"
$policyKey = "HKLM:\SOFTWARE\Prismestra\Installer\$appId"
$policyDirectory = Join-Path ([Environment]::GetFolderPath('CommonApplicationData')) (
    'Prismestra\Policies\' + $identity.User.Value)
$taskName = 'Prismestra-Backend-' + $identity.User.Value
$startupKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$pawnKey = 'HKLM:\SYSTEM\CurrentControlSet\Services\PawnIO'
$vcKey = 'HKLM:\SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64'
$webViewKey = 'Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
$shell = Join-Path ([Environment]::SystemDirectory) 'WindowsPowerShell\v1.0\powershell.exe'

function Assert-Expected([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
}

function Get-Sha256([string]$Path) {
    (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
}

function Get-PawnIOState {
    $registry = $null
    if (Test-Path -LiteralPath $pawnKey) {
        $service = Get-Item -LiteralPath $pawnKey
        try {
            $registry = [ordered]@{}
            foreach ($name in ($service.GetValueNames() | Sort-Object)) {
                $registry[$name] = [ordered]@{
                    kind = [string]$service.GetValueKind($name)
                    value = $service.GetValue($name, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
                }
            }
        } finally { $service.Dispose() }
    }
    $driver = Get-CimInstance -ClassName Win32_SystemDriver -Filter "Name = 'PawnIO'"
    $driverState = if ($null -ne $driver) {
        [ordered]@{ state = $driver.State; started = $driver.Started; start_mode = $driver.StartMode; path = $driver.PathName }
    }
    [ordered]@{
        registry = $registry
        driver = $driverState
    }
}

function Read-RegistryVersion($Hive, $View, [string]$Key, [string]$Name) {
    $base = [Microsoft.Win32.RegistryKey]::OpenBaseKey($Hive, $View)
    try {
        $entry = $base.OpenSubKey($Key)
        if ($null -ne $entry) {
            try {
                $parsed = $null
                if ([version]::TryParse([string]$entry.GetValue($Name), [ref]$parsed) -and $parsed -gt [version]'0.0.0.0') {
                    $parsed
                }
            } finally { $entry.Dispose() }
        }
    } finally { $base.Dispose() }
}

function Get-Runtimes {
    $vc = Get-ItemProperty -LiteralPath $vcKey -ErrorAction SilentlyContinue
    $vcVersion = if ($vc.Installed -eq 1 -and $null -ne $vc.Major -and $null -ne $vc.Minor -and
        $null -ne $vc.Bld -and $null -ne $vc.Rbld) {
        ([version]::new($vc.Major, $vc.Minor, $vc.Bld, $vc.Rbld)).ToString()
    }
    $vcFiles = [ordered]@{}
    foreach ($name in $vcFilesRequired) {
        $file = Get-Item -LiteralPath (Join-Path ([Environment]::SystemDirectory) $name) -ErrorAction SilentlyContinue
        if ($null -ne $file -and $file.Length -gt 0) { $vcFiles[$name] = $file.VersionInfo.FileVersion }
    }
    $webView = [ordered]@{}
    foreach ($entry in @(
        @{ scope = 'machine'; hive = 'LocalMachine'; view = 'Registry32'; base = [Environment]::GetFolderPath('ProgramFilesX86') },
        @{ scope = 'user'; hive = 'CurrentUser'; view = 'Default'; base = [Environment]::GetFolderPath('LocalApplicationData') }
    )) {
        $version = Read-RegistryVersion $entry.hive $entry.view $webViewKey 'pv'
        if ($null -eq $version) { continue }
        $runtime = Join-Path $entry.base "Microsoft\EdgeWebView\Application\$version"
        $webView[$entry.scope] = [ordered]@{
            version = $version.ToString()
            files_present = (Test-Path -LiteralPath (Join-Path $runtime 'msedgewebview2.exe') -PathType Leaf) -and
                (Test-Path -LiteralPath (Join-Path $runtime 'msedge.dll') -PathType Leaf)
        }
    }
    [ordered]@{
        vc_version = $vcVersion
        vc_files = $vcFiles
        webview = $webView
    }
}

function Assert-ColdVcAbsent {
    Assert-Expected (-not (Test-Path -LiteralPath $vcKey)) (
        'The runner has x64 VC Runtime registration. Cold download requires a clean guest.')
    foreach ($name in $vcFilesRequired) {
        Assert-Expected (-not (Test-Path -LiteralPath (Join-Path ([Environment]::SystemDirectory) $name))) (
            "The runner has a shared x64 VC Runtime file ($name). Cold download requires a clean guest.")
    }
}

function Test-RuntimesReady($Snapshot, [version]$MinimumVc) {
    ($null -ne $Snapshot.vc_version -and [version]$Snapshot.vc_version -ge $MinimumVc -and
        $Snapshot.vc_files.Count -eq $vcFilesRequired.Count -and
        @($Snapshot.webview.Values | Where-Object { $_.files_present }).Count -ge 1)
}

function Assert-RuntimesRetained($Before, $After) {
    if ($null -eq $Before) { return }
    if ($null -ne $Before.vc_version) {
        Assert-Expected ($null -ne $After.vc_version -and [version]$After.vc_version -ge [version]$Before.vc_version) (
            'Uninstall removed or downgraded the shared x64 Visual C++ runtime.')
    }
    foreach ($name in $Before.vc_files.Keys) {
        Assert-Expected ($After.vc_files.Contains($name)) "Uninstall removed shared runtime file $name."
    }
    foreach ($scope in $Before.webview.Keys) {
        $prior = $Before.webview[$scope]
        if (-not $prior.files_present) { continue }
        $remaining = @($After.webview.Values | Where-Object {
            $_.files_present -and [version]$_.version -ge [version]$prior.version
        })
        Assert-Expected ($remaining.Count -ge 1) 'Uninstall removed or downgraded the shared WebView2 Runtime.'
    }
}

function Read-Startup {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Software\Microsoft\Windows\CurrentVersion\Run')
    if ($null -eq $key) { return $null }
    try { $key.GetValue('Prismestra', $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames) }
    finally { $key.Dispose() }
}

function Assert-ProductAbsent {
    foreach ($path in @($install, $arpKey, $shortcut, $policyKey, $policyDirectory)) {
        Assert-Expected (-not (Test-Path -LiteralPath $path)) "Unexpected product artifact: $path"
    }
    Assert-Expected ($null -eq (Read-Startup)) 'Unexpected Prismestra sign-in registration.'
    $scheduler = New-Object -ComObject 'Schedule.Service'
    $scheduler.Connect()
    try { $task = $scheduler.GetFolder('\').GetTask($taskName) }
    catch {
        if ($_.Exception.GetBaseException().HResult -ne -2147024894) { throw }
        return
    }
    Assert-Expected ($null -eq $task) 'Unexpected Prismestra backend task registration.'
}

function Assert-ProtectedInstall {
    $guard = Join-Path $PSScriptRoot 'guard.ps1'
    & $shell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $guard Check $install
    Assert-Expected ($LASTEXITCODE -eq 0) 'The installed files failed the Inno setup ACL and reparse checks.'
}

function Invoke-Installer([string]$Path, [string]$Options, [string]$Log) {
    $argsForInstaller = '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART ' + $Options + ' /LOG="' + $Log + '"'
    $process = Start-Process -WindowStyle Hidden -FilePath $Path -Wait -PassThru -ArgumentList $argsForInstaller
    try { $process.ExitCode } finally { $process.Dispose() }
}

function Get-RegisteredUninstaller {
    $entry = Get-ItemProperty -LiteralPath $arpKey
    Assert-Expected ($entry.DisplayName -eq 'Prismestra' -and $entry.InstallLocation.TrimEnd('\') -eq $install) (
        ("Unexpected Add or Remove Programs registration: name=$($entry.DisplayName), location=$($entry.InstallLocation)"))
    if ($entry.QuietUninstallString -notmatch '^"(?<path>[^"]+\\unins\d{3}\.exe)"(?:\s|$)') {
        throw 'The registered uninstaller path could not be verified.'
    }
    $uninstaller = [IO.Path]::GetFullPath($Matches.path)
    Assert-Expected ([IO.Path]::GetDirectoryName($uninstaller) -eq $install -and
        (Test-Path -LiteralPath $uninstaller -PathType Leaf)) 'The uninstaller is outside the verified application root.'
    $uninstaller
}

function Copy-PrerequisiteLogs([DateTime]$Since, [string]$Destination) {
    $files = @(Get-ChildItem -LiteralPath $env:TEMP -File -ErrorAction SilentlyContinue | Where-Object {
        $_.LastWriteTimeUtc -ge $Since -and $_.Name -match '^(dd_vcredist.*|dd_vcRuntime.*|msedge_installer.*|MicrosoftEdgeUpdate.*)\.log$'
    })
    $machineEdgeLog = Join-Path ([Environment]::GetFolderPath('CommonApplicationData')) 'Microsoft\EdgeUpdate\Log\MicrosoftEdgeUpdate.log'
    $edge = Get-Item -LiteralPath $machineEdgeLog -ErrorAction SilentlyContinue
    if ($null -ne $edge -and $edge.LastWriteTimeUtc -ge $Since) { $files += $edge }
    $index = 0
    foreach ($file in $files) {
        if ($file.Attributes -band [IO.FileAttributes]::ReparsePoint) { continue }
        $index++
        Copy-Item -LiteralPath $file.FullName -Destination (Join-Path $Destination "prerequisite-$index-$($file.Name)")
    }
}

$report = [IO.Path]::GetFullPath($ReportDirectory)
foreach ($artifactRoot in @($install, $policyDirectory, $userRoot)) {
    Assert-Expected ($report -ne $artifactRoot -and
        -not $report.StartsWith($artifactRoot + '\', [StringComparison]::OrdinalIgnoreCase)) (
        'The report directory must be outside Prismestra installation and policy directories.')
}
Assert-Expected (-not (Test-Path -LiteralPath $report)) 'Use a new report directory.'
$null = New-Item -ItemType Directory -Path $report
$setupLog = Join-Path $report 'setup.log'
$uninstallLog = Join-Path $report 'uninstall.log'
$started = [DateTime]::UtcNow
$failures = [Collections.Generic.List[string]]::new()
$receipt = [ordered]@{
    version = 1
    scope = 'production_installer_lifecycle'
    result = 'failed'
    previous = $null
    saved_data_sha256 = $null
    upgrade_verified = $false
    installer_sha256 = $null
    setup_exit_code = $null
    uninstall_exit_code = $null
    reboot_required = $false
    install_verified = $false
    uninstall_verified = $false
    runtimes_before = $null
    runtimes_installed = $null
    runtimes_after = $null
    pawnio_before = $null
    pawnio_after = $null
    prereq_installer_log_mentions = @()
    vc_download_verified = $false
    silent_setup_requires_webview2_terms = $false
    application_launched_by_test = $false
    network_isolation_exercised = $false
}
$setupAttempted = $false
$savedData = [ordered]@{}
$receiptName = 'offline-setup.json'
if ($PreviousInstaller) {
    $receipt.scope = 'production_upgrade_lifecycle'
    $receiptName = 'production-upgrade.json'
} elseif ($ExerciseVcDownload) {
    $receipt.scope = 'production_vc_download_lifecycle'
    $receiptName = 'vc-download.json'
}
try {
    $installerPath = (Resolve-Path -LiteralPath $Installer).Path
    Assert-Expected ([IO.Path]::GetExtension($installerPath) -ieq '.exe') 'Expected a compiled Setup EXE.'
    $receipt.installer_sha256 = Get-Sha256 $installerPath
    Assert-Expected ($receipt.installer_sha256 -ceq $Sha256.ToLowerInvariant()) 'Setup EXE SHA-256 mismatch.'
    Assert-ProductAbsent
    $receipt.runtimes_before = Get-Runtimes
    if ($ExerciseVcDownload) { Assert-ColdVcAbsent }
    $receipt.pawnio_before = Get-PawnIOState
    # No optional PawnIO, launch or autostart selections.
    $options = '/SP- /TASKS="" /RESTARTEXITCODE=3010'
    if (-not $PreviousInstaller) {
        $declinedLog = Join-Path $report 'setup-without-terms.log'
        $declinedExit = Invoke-Installer $installerPath $options $declinedLog
        Assert-Expected ($declinedExit -eq 1 -and
            (Get-Content -LiteralPath $declinedLog -Raw) -match 'Silent Setup requires /ACCEPTWEBVIEW2TERMS') (
            'Silent Setup proceeded without explicit acceptance of the WebView2 license.')
        Assert-ProductAbsent
        $receipt.silent_setup_requires_webview2_terms = $true
    }
    if ($PreviousInstaller) {
        Assert-Expected (-not (Test-Path -LiteralPath $userRoot)) 'Upgrade requires a fresh runner profile.'
        $baseline = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'upgrade-baseline.lock.json') -Raw | ConvertFrom-Json
        $previous = Get-Item -LiteralPath $PreviousInstaller
        Assert-Expected ($previous.Name -ceq $baseline.file -and $previous.Length -eq $baseline.size -and
            (Get-Sha256 $previous.FullName) -ceq $baseline.sha256) 'Previous release has an unexpected name, size or SHA-256.'
        $setupAttempted = $true
        $baselineExit = Invoke-Installer $previous.FullName $options (Join-Path $report 'previous-setup.log')
        Assert-Expected ($baselineExit -eq 0) "Previous Setup returned $baselineExit. Upgrade requires a completed installation."
        Assert-ProtectedInstall
        $previousManifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
        Assert-Expected ($previousManifest.product -ceq 'Prismestra' -and
            [string]::IsNullOrEmpty($previousManifest.data_namespace) -and
            $previousManifest.commit -ceq $baseline.commit -and $previousManifest.installer_version -ceq $baseline.version -and
            (Get-Sha256 $image) -ceq $previousManifest.app_sha256 -and
            (Get-Sha256 (Join-Path $install $backend)) -ceq $previousManifest.backend.sha256) 'Previous installed payload does not match the pinned release.'
        $null = Get-RegisteredUninstaller
        $receipt.previous = [ordered]@{
            version = $baseline.version
            commit = $baseline.commit
            installer_sha256 = $baseline.sha256
            app_sha256 = $previousManifest.app_sha256
            backend_sha256 = $previousManifest.backend.sha256
            app_local_crt = @(
                foreach ($name in $vcFilesRequired) {
                    $legacy = Join-Path $install "components/openrgb/runtime/$name"
                    Assert-Expected (Test-Path -LiteralPath $legacy -PathType Leaf) "Missing previous app-local runtime: $name"
                    [ordered]@{ name = $name; sha256 = Get-Sha256 $legacy }
                }
            )
        }
        Assert-Expected (-not (Test-Path -LiteralPath $userRoot)) 'Previous Setup touched the synthetic profile location.'
        [void][IO.Directory]::CreateDirectory((Join-Path $userRoot 'state/assets'))
        $files = [ordered]@{
            'profile.json' = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'upgrade-profile.fixture.json') -Raw
            'state/lighting-local.json' = '{"version":1,"devices":{},"monitors":{"monitors":[]},"door_brightness":80}'
            'state/assets/ci-upgrade.txt' = 'Synthetic Prismestra upgrade fixture.'
        }
        foreach ($name in $files.Keys) {
            $path = Join-Path $userRoot $name
            [IO.File]::WriteAllText($path, $files[$name], [Text.UTF8Encoding]::new($false))
            $savedData[$name] = Get-Sha256 $path
        }
        $receipt.saved_data_sha256 = $savedData
    }
    $vcBeforeCandidate = Get-Runtimes
    if ($ExerciseVcDownload) { Assert-ColdVcAbsent }
    $setupAttempted = $true
    $receipt.setup_exit_code = Invoke-Installer $installerPath ($options + ' /ACCEPTWEBVIEW2TERMS') $setupLog
    $log = if (Test-Path -LiteralPath $setupLog -PathType Leaf) { Get-Content -LiteralPath $setupLog -Raw } else { '' }
    $receipt.reboot_required = $receipt.setup_exit_code -in @(8, 1641, 3010) -or
        $log -match 'Restart Windows to complete|exit code (1641|3010)\b'
    $receipt.prereq_installer_log_mentions = @([regex]::Matches(
        $log, '(?im)\b(vc_redist\.x64\.exe|MicrosoftEdgeWebView2RuntimeInstallerX64\.exe) exit code (\d+)'
    ) | ForEach-Object { $_.Value })
    Assert-Expected ($receipt.setup_exit_code -in @(0, 3010)) "Setup returned $($receipt.setup_exit_code)."
    Assert-Expected (Test-Path -LiteralPath $setupLog -PathType Leaf) 'Setup did not write its log.'
    Assert-Expected ($log -notmatch '(?im)\bPawnIO_setup\.exe exit code') 'Optional PawnIO installer ran.'
    $receipt.runtimes_installed = Get-Runtimes
    Assert-ProtectedInstall
    Assert-Expected (Test-Path -LiteralPath (Join-Path $install 'licenses/Microsoft-WebView2.txt') -PathType Leaf) (
        'The installed WebView2 license and privacy notice is missing.')
    $receipt.manifest_sha256 = Get-Sha256 $manifestPath
    Assert-Expected ($receipt.manifest_sha256 -ceq $ManifestSha256) 'Installed manifest SHA-256 differs from the candidate.'
    $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
    Assert-Expected ($manifest.version -eq 1 -and $manifest.product -ceq 'Prismestra' -and
        [string]::IsNullOrEmpty($manifest.data_namespace) -and $manifest.commit -ceq $Commit -and
        'vc_redist_x64' -cnotin $manifest.prerequisites.PSObject.Properties.Name -and
        $manifest.vc_runtime_download.file -ceq 'vc_redist.x64.exe' -and
        $manifest.vc_runtime_download.sha256 -cmatch '^[a-f0-9]{64}$' -and
        $manifest.vc_runtime_download.source -clike 'https://download.visualstudio.microsoft.com/download/pr/*' -and
        $manifest.prerequisites.webview2_x64.file -ceq 'MicrosoftEdgeWebView2RuntimeInstallerX64.exe' -and
        $manifest.prerequisites.webview2_x64.sha256 -cmatch '^[a-f0-9]{64}$' -and
        $manifest.prerequisites.webview2_x64.size -ge 30MB) 'Expected a production manifest with downloadable VC Runtime and embedded WebView2.'
    $receipt.commit = $manifest.commit
    $receipt.app_sha256 = Get-Sha256 $image
    $receipt.backend_sha256 = Get-Sha256 (Join-Path $install $backend)
    Assert-Expected ($receipt.app_sha256 -ceq $manifest.app_sha256 -and
        $receipt.backend_sha256 -ceq $manifest.backend.sha256 -and $manifest.backend.relative_executable -ceq $backend) (
        'Installed app or OpenRGB differs from the package SHA-256 pins.')
    $receipt.app_local_crt = @(Get-ChildItem -LiteralPath $install -Recurse -File -Force |
        Where-Object { $_.Name -in $knownAppLocalCrt } | ForEach-Object Name)
    Assert-Expected ($receipt.app_local_crt.Count -eq 0) 'Setup left an app-local Visual C++ runtime in the protected installation.'
    $minimumVc = [version]$manifest.minimum_vc_runtime
    $receipt.vc_download_verified = $log -match '(?m)Verified Microsoft Visual C\+\+ Runtime download\.'
    $vcNeeded = [string]::IsNullOrEmpty($vcBeforeCandidate.vc_version) -or
        [version]$vcBeforeCandidate.vc_version -lt $minimumVc -or
        $vcBeforeCandidate.vc_files.Count -ne $vcFilesRequired.Count
    Assert-Expected ($receipt.vc_download_verified -eq $vcNeeded) 'Setup did not follow the expected VC Runtime download/reuse path.'
    if ($ExerciseVcDownload) {
        Assert-Expected ($receipt.vc_download_verified -and -not $receipt.reboot_required -and
            $receipt.runtimes_installed.vc_version -ceq $manifest.vc_runtime_download.version -and
            $log -match '(?im)\bvc_redist\.x64\.exe exit code 0\b') (
            'The candidate did not download and install the exact VC Runtime before installing Prismestra.')
    }
    Assert-Expected (Test-RuntimesReady $receipt.runtimes_installed $minimumVc) (
        'Installed shared runtime registration or runtime binaries are missing.')
    $null = Get-RegisteredUninstaller
    Assert-Expected (Test-Path -LiteralPath $shortcut -PathType Leaf) 'Common Start menu shortcut is missing.'
    $link = (New-Object -ComObject 'WScript.Shell').CreateShortcut($shortcut)
    Assert-Expected ($link.TargetPath -eq $image -and $link.WorkingDirectory -eq $install -and
        $link.Arguments -ceq '--installed') 'Start menu shortcut does not target the normal installed app.'
    Assert-Expected ($null -eq (Read-Startup)) 'Setup selected autostart unexpectedly.'
    if ($PreviousInstaller) {
        Assert-Expected (-not $receipt.reboot_required -and
            [version]$manifest.installer_version -gt [version]$baseline.version) 'Production upgrade needs a completed, higher installer version.'
        foreach ($name in $savedData.Keys) {
            Assert-Expected ((Get-Sha256 (Join-Path $userRoot $name)) -ceq $savedData[$name]) "Upgrade changed synthetic data: $name"
        }
        $receipt.upgrade_verified = $true
    }
    $receipt.installer_version = $manifest.installer_version
    $receipt.install_verified = $true
} catch {
    $failures.Add($_.Exception.Message)
} finally {
    if ($setupAttempted) {
        try {
            if (Test-Path -LiteralPath $arpKey) {
                Assert-ProtectedInstall
                $receipt.uninstall_exit_code = Invoke-Installer (Get-RegisteredUninstaller) '' $uninstallLog
                Assert-Expected ($receipt.uninstall_exit_code -eq 0) "Uninstaller returned $($receipt.uninstall_exit_code)."
                Assert-Expected (Test-Path -LiteralPath $uninstallLog -PathType Leaf) 'Uninstaller did not write its log.'
                Assert-ProductAbsent
                foreach ($name in $savedData.Keys) {
                    Assert-Expected ((Get-Sha256 (Join-Path $userRoot $name)) -ceq $savedData[$name]) "Uninstall changed synthetic data: $name"
                }
                $receipt.uninstall_verified = $true
            } else {
                Assert-ProductAbsent
            }
        } catch { $failures.Add($_.Exception.Message) }
        try {
            $receipt.runtimes_after = Get-Runtimes
            Assert-RuntimesRetained $receipt.runtimes_before $receipt.runtimes_after
            Assert-RuntimesRetained $receipt.runtimes_installed $receipt.runtimes_after
        } catch { $failures.Add($_.Exception.Message) }
        try {
            $receipt.pawnio_after = Get-PawnIOState
            Assert-Expected (($receipt.pawnio_before | ConvertTo-Json -Depth 8 -Compress) -ceq
                ($receipt.pawnio_after | ConvertTo-Json -Depth 8 -Compress)) 'PawnIO service registration or state changed.'
        } catch { $failures.Add($_.Exception.Message) }
        try { Copy-PrerequisiteLogs $started $report } catch { $failures.Add($_.Exception.Message) }
    }
    if ($failures.Count -eq 0 -and $receipt.install_verified -and $receipt.uninstall_verified) {
        $receipt.result = if ($receipt.reboot_required) { 'passed_with_restart_pending' } else { 'passed' }
    } elseif ($receipt.reboot_required) {
        $receipt.result = 'reboot_required_incomplete'
    }
    $receipt.errors = $failures.ToArray()
    $receipt | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath (Join-Path $report $receiptName) -Encoding UTF8
}

if ($receipt.result -notin @('passed', 'passed_with_restart_pending')) {
    throw "Offline Setup lifecycle result: $($receipt.result). See $receiptName and logs in $report."
}
