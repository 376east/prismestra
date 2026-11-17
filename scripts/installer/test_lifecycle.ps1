[CmdletBinding()]
param(
    [Parameter(Mandatory)][guid]$Namespace,
    [Parameter(Mandatory)][string]$FirstBuild,
    [Parameter(Mandatory)][string]$UpgradeBuild,
    [Parameter(Mandatory)][string]$ReportDirectory,
    [Alias('NonInteractiveRunner')][switch]$ElevatedFilesOnly
)

$ErrorActionPreference = 'Stop'
$id = $Namespace.ToString('D')
$install = Join-Path $env:ProgramFiles "Prismestra Lifecycle Test\$id"
$user = Join-Path $env:LOCALAPPDATA "Prismestra\Instances\$id"
$image = Join-Path $install 'Prismestra.exe'
$profile = Join-Path $user 'profile.json'
$bindings = Join-Path $user 'state\lighting-local.json'
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$runName = "Prismestra-Test-$id"
$runValue = '"' + $image + '" --installed --autostart'
$arp = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\{' + $id + '}_is1'
$shortcut = Join-Path ([Environment]::GetFolderPath('CommonPrograms')) "Prismestra Lifecycle Test $id.lnk"
$platform = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
if ($platform -ne 'X64' -or $Namespace -eq [guid]::Empty) { throw 'Use a new test GUID and Windows x64.' }
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$elevated = [Security.Principal.WindowsPrincipal]::new($identity).IsInRole(
    [Security.Principal.WindowsBuiltInRole]::Administrator)
if ($elevated -and -not $ElevatedFilesOnly) {
    throw 'Start this check from an ordinary interactive shell so Setup can return to the original user.'
}
if ($ElevatedFilesOnly -and -not $elevated) { throw 'ElevatedFilesOnly requires an elevated disposable runner.' }

function Assert-Expected($Condition, $Message) {
    if (-not $Condition) { throw $Message }
}
function Sha256($Path) { (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant() }
function Read-TestInstaller($BuildDirectory) {
    $root = (Resolve-Path -LiteralPath $BuildDirectory).Path
    $manifestPath = Join-Path $root 'stage\payload\install-manifest.json'
    $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
    $receipt = Get-Content -LiteralPath (Join-Path $root 'installer-receipt.json') -Raw | ConvertFrom-Json
    Assert-Expected ($manifest.data_namespace -ceq $id) 'Installer belongs to a different namespace.'
    Assert-Expected (@($manifest.prerequisites.PSObject.Properties).Count -eq 0) 'Test installer contains prerequisites.'
    Assert-Expected (-not (Test-Path -LiteralPath (Join-Path $root 'stage\payload\installer_policy.ps1'))) 'Test installer contains a policy provisioner.'
    Assert-Expected ($receipt.scope -eq 'compiled_only' -and $receipt.manifest_sha256 -ceq (Sha256 $manifestPath)) 'Build receipt does not match manifest.'
    $installer = $receipt.installer
    Assert-Expected ($installer -cmatch ('^stage/dist/Prismestra-[0-9.]+-[0-9a-f]{12}-x64-test-' + $id + '\.exe$')) 'Expected an isolated test installer.'
    $exe = Join-Path $root $installer
    Assert-Expected ((Sha256 $exe) -ceq $receipt.installer_sha256) 'Build receipt does not match installer.'
    [pscustomobject]@{ Path = $exe; Manifest = $manifest }
}
function Run-Setup($Path, $Options, $Log) {
    $process = Start-Process -WindowStyle Hidden -FilePath $Path -Wait -PassThru -ArgumentList (
        '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART ' + $Options + ' /LOG="' + $Log + '"')
    Assert-Expected ($process.ExitCode -eq 0) ("Setup exited with code " + $process.ExitCode + ". See $Log")
}
function Check-Run($Expected) {
    $value = Get-ItemPropertyValue -LiteralPath $runKey -Name $runName -ErrorAction SilentlyContinue
    Assert-Expected ($value -ceq $Expected) 'The isolated per-user sign-in command did not match.'
}
function Check-Payload($Expected) {
    $actual = Get-Content -LiteralPath (Join-Path $install 'install-manifest.json') -Raw | ConvertFrom-Json
    Assert-Expected ($actual.data_namespace -ceq $id -and $actual.commit -ceq $Expected.commit -and
        $actual.installer_version -ceq $Expected.installer_version) 'Installed manifest differs from expected test build.'
    Assert-Expected ((Sha256 $image) -ceq $Expected.app_sha256) 'Installed app pin mismatch.'
    $backend = Join-Path $install 'components\openrgb\runtime\OpenRGB.exe'
    Assert-Expected ((Sha256 $backend) -ceq $Expected.backend.sha256) 'Installed backend pin mismatch.'
}

$first = Read-TestInstaller $FirstBuild
$upgrade = Read-TestInstaller $UpgradeBuild
Assert-Expected ([version]$upgrade.Manifest.installer_version -gt [version]$first.Manifest.installer_version) 'Build UpgradeBuild with a higher installer version.'
foreach ($path in @($install, $user, $arp, $shortcut, $ReportDirectory)) {
    Assert-Expected (-not (Test-Path -LiteralPath $path)) "Test requires an unused namespace and report directory: $path"
}
Check-Run $null
$report = [IO.Directory]::CreateDirectory([IO.Path]::GetFullPath($ReportDirectory)).FullName

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class PrismestraLifecycleWindow {
    delegate bool EnumProc(IntPtr window, IntPtr context);
    [DllImport("user32.dll")]
    static extern bool EnumWindows(EnumProc callback, IntPtr context);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    static extern int GetClassName(IntPtr window, StringBuilder name, int max);
    [DllImport("user32.dll")]
    static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);
    public static bool Ready(uint pid) {
        bool found = false;
        EnumWindows((window, context) => {
            uint owner;
            GetWindowThreadProcessId(window, out owner);
            if (owner != pid) return true;
            var name = new StringBuilder(64);
            GetClassName(window, name, name.Capacity);
            found = name.ToString() == "PrismestraSessionShell";
            return !found;
        }, IntPtr.Zero);
        return found;
    }
}
'@
function Start-TestSession {
    $process = Start-Process -WindowStyle Hidden -FilePath $image -ArgumentList '--installed', '--autostart' -PassThru
    $until = [DateTime]::UtcNow.AddSeconds(30)
    while ([DateTime]::UtcNow -lt $until) {
        $process.Refresh()
        Assert-Expected (-not $process.HasExited) 'Test frontend exited before creating its resident shell.'
        if ([PrismestraLifecycleWindow]::Ready($process.Id)) { return $process }
        Start-Sleep -Milliseconds 200
    }
    throw 'Timed out waiting for the installed resident shell.'
}
function Stop-TestSession($Process) {
    $stop = Start-Process -WindowStyle Hidden -FilePath $image -ArgumentList '--shutdown-installed' -PassThru -Wait
    Assert-Expected ($stop.ExitCode -eq 0 -and $Process.WaitForExit(5000)) 'Graceful installed shutdown failed.'
}

Run-Setup $first.Path '/TASKS=autostart' (Join-Path $report 'install.log')
Check-Payload $first.Manifest
Assert-Expected (Test-Path -LiteralPath $arp) 'Installer did not register the isolated uninstaller.'
Assert-Expected (Test-Path -LiteralPath $shortcut) 'Installer did not create the isolated Start menu entry.'
$asset = Join-Path $user 'state\assets\lifecycle-sentinel.txt'
if ($ElevatedFilesOnly) {
    Check-Run $runValue
    [void][IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($asset))
    [IO.File]::WriteAllText($profile, '{ "lifecycle_fixture": "' + $id + '" }')
    [IO.File]::WriteAllText($bindings, '{ "lifecycle_fixture": "' + $id + '" }')
    [IO.File]::WriteAllText((Join-Path $install 'install-manifest.json'), 'intentionally damaged lifecycle fixture')
} else {
    Check-Run $runValue
    $seed = Start-TestSession
    Stop-TestSession $seed
    $document = Get-Content -LiteralPath $profile -Raw | ConvertFrom-Json
    Assert-Expected ($document.lighting.setup.devices.Count -eq 0) 'Fresh installed profile has hardware bindings.'
    $document.lighting.scene.brightness = 42
    [IO.File]::WriteAllText($profile, ($document | ConvertTo-Json -Depth 64), [Text.UTF8Encoding]::new($false))
}
$profileHash = Sha256 $profile
$bindingsHash = Sha256 $bindings
[IO.File]::WriteAllText($asset, "Prismestra lifecycle $id", [Text.UTF8Encoding]::new($false))
$assetHash = Sha256 $asset

if (-not $ElevatedFilesOnly) { $old = Start-TestSession }
Run-Setup $upgrade.Path '/TASKS=autostart' (Join-Path $report 'upgrade.log')
if (-not $ElevatedFilesOnly) {
    Assert-Expected ($old.WaitForExit(5000)) 'Upgrade did not drain the original installed process.'
    Check-Run $runValue
} else { Check-Run $runValue }
Check-Payload $upgrade.Manifest
Assert-Expected ((Sha256 $profile) -ceq $profileHash -and (Sha256 $bindings) -ceq $bindingsHash -and
    (Sha256 $asset) -ceq $assetHash) 'Upgrade changed saved profile, local bindings or assets.'

if (-not $ElevatedFilesOnly) { $new = Start-TestSession }
$uninstallCommand = (Get-ItemProperty -LiteralPath $arp).QuietUninstallString
Assert-Expected ($uninstallCommand -match '^"(.+\\unins\d{3}\.exe)"') 'Expected the isolated registered uninstaller.'
$uninstaller = [IO.Path]::GetFullPath($Matches[1])
Assert-Expected ([IO.Path]::GetDirectoryName($uninstaller) -ceq $install) 'Uninstaller is outside the isolated test directory.'
Run-Setup $uninstaller '' (Join-Path $report 'uninstall.log')
if (-not $ElevatedFilesOnly) {
    Assert-Expected ($new.WaitForExit(5000)) 'Uninstall did not drain the installed process.'
}
foreach ($path in @($image, (Join-Path $install 'install-manifest.json'), (Join-Path $install 'components'), $arp, $shortcut)) {
    Assert-Expected (-not (Test-Path -LiteralPath $path)) "Uninstaller left an installed file or registration: $path"
}
Check-Run $null
Assert-Expected ((Sha256 $profile) -ceq $profileHash -and (Sha256 $bindings) -ceq $bindingsHash -and
    (Sha256 $asset) -ceq $assetHash) 'Uninstall changed saved profile, local bindings or assets.'

[ordered]@{
    namespace = $id
    first_version = $first.Manifest.installer_version
    upgraded_version = $upgrade.Manifest.installer_version
    startup_registry = 'exact_command_verified_in_original_hive'
    damaged_manifest_repair = [bool]$ElevatedFilesOnly
    graceful_upgrade = if ($ElevatedFilesOnly) { 'no_live_session_exercised' } else { 'resident_process_drained' }
    graceful_uninstall = if ($ElevatedFilesOnly) { 'no_live_session_exercised' } else { 'resident_process_drained' }
    profile_sha256 = $profileHash
    bindings_sha256 = $bindingsHash
    asset_sha256 = $assetHash
} | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $report 'lifecycle.json') -Encoding UTF8
