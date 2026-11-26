[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Installer,
    [Parameter(Mandatory)][ValidatePattern('^[0-9a-f]{64}$')][string]$InstallerSha256,
    [Parameter(Mandatory)][ValidatePattern('^[0-9a-f]{64}$')][string]$ManifestSha256,
    [Parameter(Mandatory)][ValidatePattern('^[0-9a-f]{40}$')][string]$Commit,
    [Parameter(Mandatory)][string]$ReportDirectory,
    [string]$WebViewProbe,
    [ValidatePattern('^[0-9a-f]{64}$')][string]$WebViewProbeSha256,
    [ValidateRange(0, 2)][int]$DdpDevices = 0,
    [switch]$DisposableRunner
)

$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -cne 'true' -or -not $DisposableRunner) {
    throw 'Run only on a disposable GitHub Actions Windows runner with -DisposableRunner.'
}
if ([Environment]::OSVersion.Platform -ne 'Win32NT' -or -not [Environment]::Is64BitProcess) {
    throw 'An x64 Windows PowerShell process is required.'
}
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
if ($identity.IsSystem -or -not [Security.Principal.WindowsPrincipal]::new($identity).IsInRole(
    [Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'The runner must use an elevated interactive-logon account.'
}

function Assert-Expected($Condition, $Message) { if (-not $Condition) { throw $Message } }
function Sha256($Path) { (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant() }
function Run-Setup($Path, $Options, $Log) {
    $process = Start-Process -FilePath $Path -Wait -PassThru -WindowStyle Hidden -ArgumentList (
        '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART ' + $Options + ' /LOG="' + $Log + '"')
    Assert-Expected ($process.ExitCode -eq 0) ("Setup exited with code " + $process.ExitCode + ". See $Log")
}

$id = [guid]::NewGuid().ToString('D')
$install = Join-Path ([Environment]::GetFolderPath('ProgramFiles')) 'Prismestra'
$probeDirectory = Join-Path (Split-Path -Parent $install) "Prismestra CI Probe-$id"
$registry = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\{9bb22971-3e88-434d-b212-ab8cf717799a}_is1'
$setup = (Resolve-Path -LiteralPath $Installer).Path
Assert-Expected ([IO.Path]::GetFileName($setup) -cmatch '^Prismestra-Setup-[0-9]+(?:\.[0-9]+){2,3}-x64\.exe$') 'Expected the production Setup.'
$setupHash = Sha256 $setup
Assert-Expected ($setupHash -ceq $InstallerSha256) 'Production Setup SHA-256 mismatch.'

$username = 'PrismCi' + ([guid]::NewGuid().ToString('N')).Substring(0, 12)
$scratch = Join-Path $env:ProgramData ("Prismestra-StandardUser-" + [guid]::NewGuid().ToString('N'))
foreach ($path in @($install, $registry, $scratch, $probeDirectory, $ReportDirectory)) {
    if ($path -eq $install -and (Test-Path -LiteralPath $path -PathType Container) -and
        @(Get-ChildItem -LiteralPath $path -Force).Count -eq 0) { continue }
    Assert-Expected (-not (Test-Path -LiteralPath $path)) ("Expected an unused namespace and output directory: " + $path)
}
Assert-Expected (-not (Get-LocalUser -Name $username -ErrorAction SilentlyContinue)) 'Fixture username already exists.'
$report = [IO.Directory]::CreateDirectory([IO.Path]::GetFullPath($ReportDirectory)).FullName

# A private window station avoids changing existing desktop ACLs. A second
# logon loads the disposable account's own environment and registry hive.
Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Security;
using System.Text;
public static class PrismestraCiLogon {
    [StructLayout(LayoutKind.Sequential)]
    struct SecurityAttributes { public int Length; public IntPtr Descriptor; public int Inherit; }
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct Startup {
        public int Size; public string Reserved, Desktop, Title;
        public int X, Y, XSize, YSize, XChars, YChars, Fill, Flags;
        public short Show, ReservedBytes;
        public IntPtr ReservedData, Input, Output, Error;
    }
    [StructLayout(LayoutKind.Sequential)]
    struct ProcessInformation { public IntPtr Process, Thread; public int Pid, Tid; }
    [DllImport("advapi32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
    static extern bool ConvertStringSecurityDescriptorToSecurityDescriptor(
        string sddl, uint version, out IntPtr descriptor, IntPtr size);
    [DllImport("user32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
    static extern IntPtr CreateWindowStation(string name, uint flags, uint access, ref SecurityAttributes sa);
    [DllImport("user32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
    static extern IntPtr CreateDesktop(string name, IntPtr device, IntPtr mode, uint flags, uint access, ref SecurityAttributes sa);
    [DllImport("user32.dll")] static extern IntPtr GetProcessWindowStation();
    [DllImport("user32.dll", SetLastError=true)] static extern bool SetProcessWindowStation(IntPtr station);
    [DllImport("user32.dll")] static extern bool CloseWindowStation(IntPtr station);
    [DllImport("user32.dll")] static extern bool CloseDesktop(IntPtr desktop);
    [DllImport("advapi32.dll", CharSet=CharSet.Unicode, ExactSpelling=true, SetLastError=true)]
    static extern bool CreateProcessWithLogonW(string user, string domain, IntPtr password, uint logonFlags,
        string image, StringBuilder command, uint flags, IntPtr environment, string cwd,
        ref Startup startup, out ProcessInformation information);
    [DllImport("kernel32.dll")] static extern IntPtr LocalFree(IntPtr ptr);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
    [DllImport("kernel32.dll")] static extern uint WaitForSingleObject(IntPtr handle, uint milliseconds);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool GetExitCodeProcess(IntPtr process, out int code);
    [DllImport("kernel32.dll")] static extern bool TerminateProcess(IntPtr process, uint code);
    static void Require(bool result, string operation) {
        if (!result) throw new Win32Exception(Marshal.GetLastWin32Error(), operation);
    }
    public static int Run(string user, SecureString password, string sid, string image, string args, string cwd) {
        IntPtr sd = IntPtr.Zero, station = IntPtr.Zero, desktop = IntPtr.Zero;
        IntPtr prior = GetProcessWindowStation();
        IntPtr secret = IntPtr.Zero;
        ProcessInformation child = new ProcessInformation();
        string stationName = "PrismestraCI" + Guid.NewGuid().ToString("N");
        try {
            Require(ConvertStringSecurityDescriptorToSecurityDescriptor(
                "O:BAG:BAD:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;" + sid + ")",
                1, out sd, IntPtr.Zero), "Create private desktop ACL");
            var sa = new SecurityAttributes { Length = Marshal.SizeOf(typeof(SecurityAttributes)), Descriptor = sd };
            station = CreateWindowStation(stationName, 0, 0x000f037f, ref sa);
            Require(station != IntPtr.Zero, "Create private window station");
            Require(SetProcessWindowStation(station), "Select private window station");
            try {
                desktop = CreateDesktop("Default", IntPtr.Zero, IntPtr.Zero, 0, 0x10000000, ref sa);
                Require(desktop != IntPtr.Zero, "Create private desktop");
            } finally { Require(SetProcessWindowStation(prior), "Restore runner window station"); }
            var startup = new Startup { Size = Marshal.SizeOf(typeof(Startup)), Desktop = stationName + "\\Default" };
            secret = Marshal.SecureStringToGlobalAllocUnicode(password);
            Require(CreateProcessWithLogonW(user, ".", secret, 1, image,
                new StringBuilder("\"" + image + "\" " + args, 1024), 0, IntPtr.Zero, cwd,
                ref startup, out child), "Log in fixture account with its own profile");
            Marshal.ZeroFreeGlobalAllocUnicode(secret);
            secret = IntPtr.Zero;
            if (WaitForSingleObject(child.Process, 240000) != 0) {
                TerminateProcess(child.Process, 1);
                throw new TimeoutException("Standard-user fixture worker did not finish.");
            }
            int code;
            Require(GetExitCodeProcess(child.Process, out code), "Read fixture worker exit status");
            return code;
        } finally {
            if (secret != IntPtr.Zero) Marshal.ZeroFreeGlobalAllocUnicode(secret);
            if (child.Thread != IntPtr.Zero) CloseHandle(child.Thread);
            if (child.Process != IntPtr.Zero) CloseHandle(child.Process);
            if (desktop != IntPtr.Zero) CloseDesktop(desktop);
            if (station != IntPtr.Zero) CloseWindowStation(station);
            if (sd != IntPtr.Zero) LocalFree(sd);
        }
    }
}
'@

$worker = @'
param($Install, $Id, $ExpectedSid, [int]$DdpDevices)
$ErrorActionPreference = 'Stop'
$env:PSModulePath = Join-Path $PSHOME Modules
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$admin = [Security.Principal.WindowsPrincipal]::new($identity).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
$local = [Environment]::GetFolderPath('LocalApplicationData')
if ($identity.User.Value -ne $ExpectedSid -or $admin -or $local -cne $env:LOCALAPPDATA) { exit 2 }
$receiptPath = Join-Path $local ("Prismestra-CI-" + $Id + ".json")
$result = [ordered]@{ sid = $identity.User.Value; standard_user = $true; startup_command = $null;
    empty_profile = $false; local_bindings = $false; backend_pin = $false; resident_shell = 'not_observed';
    automatic_setup = $false; first_run = $null; native_identity_probe = $false; native_profile_prepare = $false;
    native_current_sid = $false; native_startup = $false; native_webview_probe = $false;
    native_bootstrap = $false; seeded_ddp_devices = $DdpDevices; first_scene = $null }
$app = $null
$stop = $null
$receivers = @()
try {
    $profile = Join-Path $local 'Prismestra'
    if (Test-Path -LiteralPath $profile) { throw 'The standard-user profile was not fresh.' }
    $probePath = Join-Path (Join-Path (Split-Path -Parent $Install) "Prismestra CI Probe-$Id") 'Prismestra-Setup-Probe.exe'
    if (Test-Path -LiteralPath $probePath) {
        function Assert-UserWebViewProbe([int]$Expected) {
            $child = Start-Process -FilePath $probePath -PassThru -WindowStyle Hidden
            $null = $child.Handle
            try {
                if (-not $child.WaitForExit(20000)) { $child.Kill(); throw 'WebView2 probe timed out.' }
                if ($child.ExitCode -ne $Expected) {
                    throw ('WebView2 probe returned ' + $child.ExitCode + ' instead of ' + $Expected)
                }
            } finally { $child.Dispose() }
        }
        $webView = 'Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
        Assert-UserWebViewProbe 1
        $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($webView)
        $changed = $false
        try {
            if ($null -ne $key.GetValue('pv')) { throw 'Disposable user already has a WebView2 version value.' }
            foreach ($case in @(@{v='104.65535.65535.65535'; exit=1}, @{v='105.0.0.0'; exit=0}, @{v='105.0.0'; exit=1})) {
                $key.SetValue('pv', $case.v, [Microsoft.Win32.RegistryValueKind]::String)
                $changed = $true
                Assert-UserWebViewProbe $case.exit
            }
        } finally {
            if ($changed) { $key.DeleteValue('pv', $false) }
            $key.Dispose()
        }
        $result.native_webview_probe = $true
    }
    $image = Join-Path $Install 'Prismestra.exe'
    $currentSidOutput = Join-Path $PSScriptRoot 'native-current-sid.log'
    $sidProbe = Start-Process -FilePath $image -ArgumentList '--setup-current-user' -PassThru -WindowStyle Hidden -RedirectStandardOutput $currentSidOutput
    $null = $sidProbe.Handle
    try {
        if (-not $sidProbe.WaitForExit(20000)) { $sidProbe.Kill(); throw 'Native SID read timed out.' }
        if ($sidProbe.ExitCode -ne 0 -or (Get-Content -LiteralPath $currentSidOutput -Raw).Trim() -cne $ExpectedSid) {
            throw 'Native SID read did not return the standard user.'
        }
    } finally { $sidProbe.Dispose() }
    $result.native_current_sid = $true
    foreach ($case in @(@{sid=$ExpectedSid; exit=0}, @{sid='S-1-5-18'; exit=1})) {
        $probe = Start-Process -FilePath $image -ArgumentList '--setup-check-user', $case.sid -PassThru -WindowStyle Hidden
        $null = $probe.Handle
        try {
            if (-not $probe.WaitForExit(20000)) { $probe.Kill(); throw 'Native identity probe timed out.' }
            if ($probe.ExitCode -ne $case.exit) { throw 'Native identity probe returned an unexpected exit code.' }
        } finally { $probe.Dispose() }
    }
    $result.native_identity_probe = $true
    $wrongUser = Start-Process -FilePath $image -ArgumentList '--setup-prepare-installed', 'S-1-5-18' -Wait -PassThru -WindowStyle Hidden
    if ($wrongUser.ExitCode -ne 1 -or (Test-Path -LiteralPath $profile)) { throw 'Profile preparation accepted a foreign SID.' }
    $value = 'Prismestra'
    $command = '"' + (Join-Path $Install Prismestra.exe) + '" --installed --autostart'
    $runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
    function Invoke-Startup([string]$Arg, [int]$Expected, [string]$Executable = $image) {
        $stderr = Join-Path $PSScriptRoot 'native-startup.stderr.log'
        $helper = Start-Process -FilePath $Executable -ArgumentList $Arg -PassThru -WindowStyle Hidden -RedirectStandardError $stderr
        $null = $helper.Handle
        try {
            if (-not $helper.WaitForExit(20000)) { $helper.Kill(); throw 'Native startup operation timed out.' }
            if ($helper.ExitCode -ne $Expected) {
                $diagnostic = ([string](Get-Content -LiteralPath $stderr -Raw)).Trim()
                throw ('Native startup operation returned ' + $helper.ExitCode + ' instead of ' + $Expected + ': ' + $diagnostic)
            }
        } finally {
            $helper.Dispose()
            Remove-Item -LiteralPath $stderr -ErrorAction SilentlyContinue
        }
    }
    [void](New-Item -Path $runKey -Force)
    $foreign = '%TEMP%\disposable-fixture-only.exe'
    [void](New-ItemProperty -Path $runKey -Name $value -Value $foreign -PropertyType ExpandString -Force)
    Invoke-Startup '--setup-enable-startup' 2
    Invoke-Startup '--setup-disable-startup' 0
    $raw = (Get-Item -LiteralPath $runKey).GetValue($value, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    if ($raw -cne $foreign) { throw 'Native startup did not preserve the foreign value.' }
    if ($result.native_webview_probe) {
        $sidProbe = Start-Process -FilePath $probePath -ArgumentList '--current-sid' -Wait -PassThru -WindowStyle Hidden -RedirectStandardOutput $currentSidOutput
        try {
            if ($sidProbe.ExitCode -ne 0 -or (Get-Content -LiteralPath $currentSidOutput -Raw).Trim() -cne $ExpectedSid) {
                throw 'Static bootstrap SID read did not return the standard user.'
            }
        } finally { $sidProbe.Dispose() }
        Invoke-Startup ('--check-user ' + $ExpectedSid) 0 $probePath
        Invoke-Startup '--check-user S-1-5-18' 1 $probePath
        Invoke-Startup ('--enable-startup "' + $image + '"') 2 $probePath
        Invoke-Startup ('--disable-startup "' + $image + '"') 0 $probePath
        if ((Get-Item -LiteralPath $runKey).GetValue($value, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames) -cne $foreign) {
            throw 'Static bootstrap did not preserve the foreign startup value.'
        }
        Remove-ItemProperty -LiteralPath $runKey -Name $value
        Invoke-Startup ('--enable-startup "' + $image + '"') 0 $probePath
        if ((Get-ItemPropertyValue $runKey $value) -cne $command) { throw 'Static bootstrap wrote an unexpected startup command.' }
        Invoke-Startup ('--disable-startup "' + $image + '"') 0 $probePath
        if ($null -ne (Get-Item -LiteralPath $runKey).GetValue($value)) { throw 'Static bootstrap did not remove the owned startup value.' }
        $result.native_bootstrap = $true
    } else { Remove-ItemProperty -LiteralPath $runKey -Name $value }
    Invoke-Startup '--setup-enable-startup' 0
    Invoke-Startup '--setup-disable-startup' 0
    if ($null -ne (Get-Item -LiteralPath $runKey).GetValue($value)) { throw 'Native startup did not remove the owned value.' }
    Invoke-Startup '--setup-enable-startup' 0
    $prior = Get-ItemPropertyValue $runKey $value
    if ($prior -cne $command) { throw 'HKCU startup command did not match.' }
    $result.startup_command = $prior
    $result.native_startup = $true
    $manifest = Get-Content -LiteralPath (Join-Path $Install 'install-manifest.json') -Raw | ConvertFrom-Json
    if ($null -ne $manifest.data_namespace) { throw 'Expected the installed production manifest.' }
    $documentPath = Join-Path $profile 'profile.json'
    $configPath = Join-Path $profile 'state\managed-openrgb.json'
    $bindingPath = Join-Path $profile 'state\lighting-local.json'
    if ($DdpDevices) {
        Add-Type -Path (Join-Path $PSScriptRoot 'first_scene_receiver.cs')
        if ([PrismestraCiDdpReceiver]::SdkOwner() -ne 0 -or (Test-Path -LiteralPath $profile)) {
            throw 'Positive first-scene fixture requires an unused profile and SDK port.'
        }
        $devices = @(
            for ($i = 0; $i -lt $DdpDevices; $i++) {
                @{name = "Prismestra CI DDP $i"; ip = '127.0.0.1'; port = 14048 + $i; num_leds = 2 + $i}
            }
        )
        foreach ($device in $devices) {
            $receivers += [PrismestraCiDdpReceiver]::new($device.port, $device.num_leds)
        }
        $seedDir = Join-Path $profile 'state\openrgb'
        [void][IO.Directory]::CreateDirectory($seedDir)
        $seed = Join-Path $seedDir 'OpenRGB.json'
        @{DDPDevices = @{devices = $devices}} | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $seed -Encoding ASCII
        $before = @(Get-ChildItem -LiteralPath $profile -File -Recurse -Force)
        if ($before.Count -ne 1 -or $before[0].FullName -ine $seed) {
            throw 'Only the OpenRGB device configuration may exist before launch.'
        }
        $result.first_scene = [ordered]@{
            prelaunch_files = @('state/openrgb/OpenRGB.json')
            openrgb_ddp_devices = $devices
            ticks_per_second = [Diagnostics.Stopwatch]::Frequency
        }
        $result.empty_profile = -not (Test-Path -LiteralPath $documentPath)
        $result.local_bindings = -not (Test-Path -LiteralPath $bindingPath)
        $result.first_scene.launch_tick = [Diagnostics.Stopwatch]::GetTimestamp()
    }
    $app = Start-Process -FilePath $image -ArgumentList '--installed', '--autostart' -PassThru -WindowStyle Hidden -RedirectStandardError (Join-Path $PSScriptRoot 'app.stderr.log') -RedirectStandardOutput (Join-Path $PSScriptRoot 'app.stdout.log')
    $null = $app.Handle
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    while ([DateTime]::UtcNow -lt $deadline -and
        -not ((Test-Path -LiteralPath $documentPath) -and (Test-Path -LiteralPath $configPath) -and
            (Test-Path -LiteralPath $bindingPath))) {
        if ($app.HasExited) { throw ('App exited before writing its bootstrap files: ' + $app.ExitCode) }
        Start-Sleep -Milliseconds 200
    }
    $backend = Get-Content -LiteralPath $configPath -Raw | ConvertFrom-Json
    if (!$DdpDevices) {
        $document = Get-Content -LiteralPath $documentPath -Raw | ConvertFrom-Json
        $bindings = Get-Content -LiteralPath $bindingPath -Raw | ConvertFrom-Json
        $result.empty_profile = $document.version -eq 1 -and $document.lighting.version -eq 1 -and
            $document.lighting.setup.version -eq 1 -and $document.lighting.setup.devices.Count -eq 0 -and
            $document.lighting.setup.canvas.points.Count -eq 0 -and -not $document.lighting.schedule.enabled
        $result.local_bindings = $bindings.version -eq 1 -and @($bindings.devices.PSObject.Properties).Count -eq 0 -and
            $bindings.monitors.monitors.Count -eq 0
    }
    $relative = 'components\openrgb\runtime\OpenRGB.exe'
    $pinned = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $Install $relative)).Hash.ToLowerInvariant()
    $backendImage = $backend.executable
    $backendConfig = $backend.config_dir
    if ($backendImage.StartsWith('\\?\')) { $backendImage = $backendImage.Substring(4) }
    if ($backendConfig.StartsWith('\\?\')) { $backendConfig = $backendConfig.Substring(4) }
    $result.backend_pin = $backend.version -eq 1 -and $backend.enabled -and $backend.endpoint -ceq '127.0.0.1:17422' -and
        $backend.expected_executable_sha256 -ceq $pinned -and $manifest.backend.sha256 -ceq $pinned -and
        $backendImage -ieq (Join-Path $Install $relative) -and
        $backendConfig -ieq (Join-Path $profile 'state\openrgb')
    if (-not ($result.empty_profile -and $result.local_bindings -and $result.backend_pin)) {
        throw 'Standard-user bootstrap content did not match the installed contract.'
    }
    # Observe the real resident coordinator, not merely creation of empty JSON files.
    $automaticPath = Join-Path $profile 'state\first-run-status.json'
    $until = [DateTime]::UtcNow.AddSeconds(65)
    while (-not $app.HasExited -and [DateTime]::UtcNow -lt $until) {
        if (Test-Path -LiteralPath $automaticPath) {
            try { $result.first_run = Get-Content -LiteralPath $automaticPath -Raw -Encoding UTF8 | ConvertFrom-Json }
            catch { $result.first_run = $null }
            if ($null -ne $result.first_run) {
                if ($result.first_run.phase -eq 'error') { throw ('Automatic setup failed: ' + $result.first_run.detail) }
                if ($result.first_run.native_outputs -and $result.first_run.phase -in @('applied','discovered','complete') -and
                    (!$DdpDevices -or $result.first_run.devices -eq $DdpDevices)) {
                    $sdkErrors = @($result.first_run.discovery_issues | Where-Object {
                        $_ -match '^OpenRGB:|^Управляемый OpenRGB недоступен'
                    })
                    if ($sdkErrors.Count) { throw ('Automatic managed SDK discovery failed: ' + ($sdkErrors -join '; ')) }
                    $result.automatic_setup = $true
                    break
                }
            }
        }
        Start-Sleep -Milliseconds 200
    }
    if (-not $result.automatic_setup) { throw 'Installed app never automatically restored and discovered outputs.' }
    if ($DdpDevices) {
        if ($result.first_run.early_manual_starts -ne 0 -or $result.first_run.process_id -ne $app.Id -or
            [long]$result.first_run.process_creation_filetime -ne $app.StartTime.ToUniversalTime().ToFileTimeUtc()) {
            throw 'Automatic setup receipt did not belong to this unassisted launch.'
        }
        $document = Get-Content -LiteralPath $documentPath -Raw | ConvertFrom-Json
        $bindings = Get-Content -LiteralPath $bindingPath -Raw | ConvertFrom-Json
        $setup = $document.lighting.setup
        $leds = ($devices | ForEach-Object { $_.num_leds } | Measure-Object -Sum).Sum
        if ($setup.devices.Count -ne $DdpDevices -or @($bindings.devices.PSObject.Properties).Count -ne $DdpDevices -or
            $setup.canvas.points.Count -ne $leds -or @($bindings.monitors.monitors).Count -ne 0) {
            throw 'Automatic setup did not persist the expected device, binding and canvas counts.'
        }
        $mapped = @(
            foreach ($device in $devices) {
                $rows = @($setup.devices | Where-Object { $_.label -ceq $device.name })
                $row = $rows[0]
                if ($rows.Count -ne 1 -or $row.id -cnotmatch '^auto-rgb-[0-9a-f]{40}$' -or
                    $row.driver.type -cne 'open_rgb' -or !$row.enabled -or $row.leds -ne $device.num_leds) {
                    throw ("Invalid automatic OpenRGB device for " + $device.name)
                }
                $points = @($setup.canvas.points | Where-Object { $_.device -ceq $row.id })
                $indices = @($points.led | Sort-Object -Unique)
                $outside = @($points | Where-Object {
                    $_.x -lt 0 -or $_.x -ge $setup.canvas.width -or $_.y -lt 0 -or $_.y -ge $setup.canvas.height
                })
                if ($points.Count -ne $device.num_leds -or ($indices -join ',') -cne ((0..($device.num_leds - 1)) -join ',') -or $outside.Count) {
                    throw ("Invalid mapped LED geometry for " + $device.name)
                }
                $assignment = $bindings.devices.PSObject.Properties[$row.id].Value
                if ($assignment.type -cne 'open_rgb' -or $assignment.endpoint -cne '127.0.0.1:17422' -or
                    $assignment.identity -cnotmatch '^[0-9a-f]{64}$' -or $assignment.topology -cnotmatch '^[0-9a-f]{64}$') {
                    throw ("Invalid local OpenRGB binding for " + $device.name)
                }
                @{id = $row.id; binding = $assignment; points = $points; port = $device.port}
            }
        )
        if (@($mapped | ForEach-Object { $_.binding.identity } | Sort-Object -Unique).Count -ne $DdpDevices) {
            throw 'Configured DDP controllers did not obtain distinct bindings.'
        }
        $result.first_scene.map = $mapped
        $result.first_scene.profile_sha256 = (Get-FileHash -LiteralPath $documentPath).Hash
        $result.first_scene.bindings_sha256 = (Get-FileHash -LiteralPath $bindingPath).Hash
        $owner = [PrismestraCiDdpReceiver]::SdkOwner()
        if ($owner -le 0) { throw 'The installed OpenRGB SDK listener was missing or ambiguous.' }
        $child = Get-Process -Id $owner
        try {
            $null = $child.Handle
            $info = Get-CimInstance Win32_Process -Filter ('ProcessId=' + $owner)
            if ($child.MainModule.FileName -ine $backendImage -or $info.ParentProcessId -ne $app.Id -or
                $child.StartTime -lt $app.StartTime) { throw 'The SDK listener was not the resident-owned OpenRGB process.' }
            $result.first_scene.backend = @{pid = $owner; parent_pid = $info.ParentProcessId; image = $backendImage;
                creation_filetime = $child.StartTime.ToUniversalTime().ToFileTimeUtc().ToString()}
            $frames = [object[]]::new($DdpDevices)
            $result.first_scene.frames = $frames
            $until = [DateTime]::UtcNow.AddSeconds(12)
            do {
                for ($i = 0; $i -lt $DdpDevices; $i++) {
                    if (!$frames[$i]) { $frames[$i] = $receivers[$i].ReadFrame($owner) }
                }
                if ($frames -notcontains $null) { break }
                Start-Sleep -Milliseconds 50
            } while (!$app.HasExited -and !$child.HasExited -and [DateTime]::UtcNow -lt $until)
            if ($frames -contains $null -or $app.HasExited -or $child.HasExited -or
                [PrismestraCiDdpReceiver]::SdkOwner() -ne $owner) {
                throw 'Missing nonblack DDP frames with expected geometry from the resident-owned OpenRGB process.'
            }
        } finally { $child.Dispose() }
    }
    try {
        Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class PrismestraCiShell {
    delegate bool EnumProc(IntPtr hwnd, IntPtr context);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc callback, IntPtr context);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)]
    static extern int GetClassName(IntPtr hwnd, StringBuilder name, int max);
    public static bool Observe(uint expected) {
        bool found = false;
        EnumWindows((hwnd, context) => {
            uint pid;
            GetWindowThreadProcessId(hwnd, out pid);
            if (pid != expected) return true;
            var name = new StringBuilder(64);
            GetClassName(hwnd, name, name.Capacity);
            found = name.ToString() == "PrismestraSessionShell";
            return !found;
        }, IntPtr.Zero);
        return found;
    }
}
"@
        $until = [DateTime]::UtcNow.AddSeconds(8)
        while (-not $app.HasExited -and [DateTime]::UtcNow -lt $until) {
            if ([PrismestraCiShell]::Observe($app.Id)) {
                $result.resident_shell = 'observed_drain_unverified'
                $stop = Start-Process -FilePath $image -ArgumentList '--shutdown-installed' -WindowStyle Hidden -PassThru
                $null = $stop.Handle
                if ($stop.WaitForExit(50000) -and $stop.ExitCode -eq 0 -and $app.WaitForExit(3000) -and $app.ExitCode -eq 0) {
                    $result.resident_shell = 'process_drained_by_shutdown'
                }
                break
            }
            Start-Sleep -Milliseconds 200
        }
    } catch { $result.resident_shell_error = $_.Exception.Message }
    if ($result.resident_shell -eq 'process_drained_by_shutdown') {
        $profileHash = (Get-FileHash -LiteralPath $documentPath).Hash
        $bindingsHash = (Get-FileHash -LiteralPath $bindingPath).Hash
        if ($DdpDevices -and ($profileHash -cne $result.first_scene.profile_sha256 -or
            $bindingsHash -cne $result.first_scene.bindings_sha256)) {
            throw 'Saved automatic map or bindings changed before the resident drained.'
        }
        if ($DdpDevices) { $result.first_scene.saved_map_stable = $true }
        $prep = Start-Process -FilePath $image -ArgumentList '--setup-prepare-installed', $ExpectedSid -Wait -PassThru -WindowStyle Hidden
        if ($prep.ExitCode -ne 0 -or (Get-FileHash -LiteralPath $documentPath).Hash -cne $profileHash -or
            (Get-FileHash -LiteralPath $bindingPath).Hash -cne $bindingsHash) {
            throw 'Native profile preparation changed the saved profile or bindings.'
        }
        $result.native_profile_prepare = $true
    }
    $result.startup_error_file = Test-Path -LiteralPath (Join-Path $profile 'state\startup-error.txt')
    $result | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $receiptPath -Encoding UTF8
    exit 0
} catch {
    $result.error = $_.Exception.Message
    $result | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $receiptPath -Encoding UTF8
    exit 1
} finally {
    # Retained handles are only for this disposable fixture's own processes.
    if ($app -and -not $app.HasExited) { $app.Kill(); [void]$app.WaitForExit(5000) }
    if ($stop -and -not $stop.HasExited) { $stop.Kill(); [void]$stop.WaitForExit(5000) }
    foreach ($receiver in $receivers) { $receiver.Dispose() }
}
'@

$account = $null
$password = $null
$setupAttempted = $false
try {
    $setupAttempted = $true
    Run-Setup $setup '/TASKS=""' (Join-Path $report 'install.log')
    $manifestPath = Join-Path $install 'install-manifest.json'
    Assert-Expected ((Sha256 $manifestPath) -ceq $ManifestSha256) 'Installed production manifest SHA-256 mismatch.'
    $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
    Assert-Expected ($manifest.commit -ceq $Commit -and $null -eq $manifest.data_namespace) 'Expected the production manifest for this commit.'
    Assert-Expected ((Sha256 (Join-Path $install 'Prismestra.exe')) -ceq $manifest.app_sha256) 'Installed production app SHA-256 mismatch.'

    $password = [Security.SecureString]::new()
    foreach ($ch in ('Q7!' + [guid]::NewGuid().ToString('N')).ToCharArray()) { $password.AppendChar($ch) }
    $password.MakeReadOnly()
    $account = New-LocalUser -Name $username -Password $password -Description 'Disposable Prismestra CI fixture'
    $sid = $account.SID.Value
    $users = [Security.Principal.SecurityIdentifier]::new('S-1-5-32-545')
    if ($sid -notin @((Get-LocalGroupMember -SID $users).SID.Value)) {
        Add-LocalGroupMember -SID $users -Member $account
    }
    [void][IO.Directory]::CreateDirectory($scratch)
    $permissions = Get-Acl -LiteralPath $scratch
    $permissions.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new(
        $account.SID, 'Modify', 'ContainerInherit,ObjectInherit', 'None', 'Allow'))
    Set-Acl -LiteralPath $scratch -AclObject $permissions
    if ($WebViewProbe) {
        [void][IO.Directory]::CreateDirectory($probeDirectory)
        $acl = [Security.AccessControl.DirectorySecurity]::new()
        $acl.SetSecurityDescriptorSddlForm('O:BAD:PAI(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)(A;OICI;FRFX;;;BU)')
        Set-Acl -LiteralPath $probeDirectory -AclObject $acl
        $stagedProbe = Join-Path $probeDirectory 'Prismestra-Setup-Probe.exe'
        Copy-Item -LiteralPath $WebViewProbe -Destination $stagedProbe
        if ((Get-FileHash -LiteralPath $stagedProbe -Algorithm SHA256).Hash.ToLowerInvariant() -cne $WebViewProbeSha256) {
            throw 'WebView2 probe differs from the original installer input.'
        }
    }
    $workerFile = Join-Path $scratch 'worker.ps1'
    if ($DdpDevices) {
        Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'first_scene_receiver.cs') -Destination $scratch
    }
    # Windows PowerShell 5.1 needs a BOM for non-ASCII source on every system locale.
    [IO.File]::WriteAllText($workerFile, $worker, [Text.UTF8Encoding]::new($true))
    $powershell = Join-Path ([Environment]::SystemDirectory) 'WindowsPowerShell\v1.0\powershell.exe'
    $workerLog = Join-Path $scratch 'worker-output.log'
    $wrapperFile = Join-Path $scratch 'invoke-worker.ps1'
    $wrapper = @'
param($Install, $Id, $ExpectedSid, $DdpDevices)
$ErrorActionPreference = 'Stop'
$log = Join-Path $PSScriptRoot 'worker-output.log'
try {
    & (Join-Path $PSScriptRoot 'worker.ps1') $Install $Id $ExpectedSid $DdpDevices *> $log
    exit $LASTEXITCODE
} catch {
    $_ | Out-String | Add-Content -LiteralPath $log -Encoding UTF8
    exit 1
}
'@
    [IO.File]::WriteAllText($wrapperFile, $wrapper, [Text.UTF8Encoding]::new($true))
    # CreateProcessWithLogonW has a 1024-character command-line limit.
    # https://learn.microsoft.com/windows/win32/api/winbase/nf-winbase-createprocesswithlogonw
    $workerArguments = '-NoProfile -NonInteractive -ExecutionPolicy Bypass -File "' + $wrapperFile + '" "' + $install + '" ' + $id + ' ' + $sid + ' ' + $DdpDevices
    Assert-Expected (($powershell.Length + $workerArguments.Length + 4) -lt 1024) 'Fixture logon command line exceeds the Windows limit.'
    $started = Get-Date
    $status = [PrismestraCiLogon]::Run($username, $password, $sid, $powershell, $workerArguments, $scratch)
    # The bundled Windows entrypoint reports failures before its file logger
    # starts to the Application event log. Preserve those own-run diagnostics.
    $events = @(Get-WinEvent -FilterHashtable @{LogName='Application'; StartTime=$started} -ErrorAction SilentlyContinue |
        Where-Object ProviderName -EQ 'OpenRGB')
    $events | Select-Object TimeCreated, Id, Message, @{Name='Values';Expression={@($_.Properties | ForEach-Object Value)}} |
        ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $report 'backend-events.json') -Encoding UTF8
    if (Test-Path -LiteralPath $workerLog) {
        Copy-Item -LiteralPath $workerLog -Destination (Join-Path $report 'worker-output.log')
        if ($status -ne 0) { Get-Content -LiteralPath $workerLog -Tail 40 | Write-Host }
    }
    foreach ($name in @('app.stderr.log','app.stdout.log')) {
        $log = Join-Path $scratch $name
        if (Test-Path -LiteralPath $log) { Copy-Item -LiteralPath $log -Destination (Join-Path $report $name) }
    }
    foreach ($name in @('startup-helper.stdout.log', 'startup-helper.stderr.log')) {
        $log = Get-Item -LiteralPath (Join-Path $scratch $name) -ErrorAction SilentlyContinue
        if ($log -and $log.Length -le 64KB) {
            Copy-Item -LiteralPath $log.FullName -Destination (Join-Path $report $name)
        } elseif ($log) {
            Write-Warning "Omitted $name because it exceeds 64 KiB."
        }
    }
    $profile = Get-CimInstance -ClassName Win32_UserProfile -Filter "SID='$sid'"
    Assert-Expected ($null -ne $profile) "Standard-user profile was not loaded (worker exit $status)."
    $backendLogs = Join-Path $profile.LocalPath 'AppData\Local\Prismestra\state\openrgb'
    if (Test-Path -LiteralPath $backendLogs) {
        $index = 0
        Get-ChildItem -LiteralPath $backendLogs -Filter '*.log' -File -Recurse |
            Where-Object Length -LE 2097152 | Select-Object -First 12 | ForEach-Object {
                Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $report ('backend-' + $index + '.log'))
                $index++
            }
    }
    $evidence = Join-Path $profile.LocalPath ("AppData\Local\Prismestra-CI-" + $id + ".json")
    Assert-Expected (Test-Path -LiteralPath $evidence) "Fixture worker did not report assertions (exit $status)."
    $actual = Get-Content -LiteralPath $evidence -Raw | ConvertFrom-Json
    $actual | Add-Member -NotePropertyName installer_sha256 -NotePropertyValue $setupHash
    $actual | Add-Member -NotePropertyName commit -NotePropertyValue $manifest.commit
    $actual | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath (Join-Path $report 'standard-user.json') -Encoding UTF8
    Assert-Expected ($status -eq 0 -and $actual.sid -ceq $sid -and $actual.standard_user -and $actual.empty_profile -and
        $actual.local_bindings -and $actual.backend_pin -and $actual.automatic_setup -and
        $actual.native_identity_probe -and $actual.native_profile_prepare -and $actual.native_current_sid -and $actual.native_startup -and
        $actual.resident_shell -eq 'process_drained_by_shutdown' -and -not $actual.startup_error_file) 'Standard-user verification failed. See standard-user.json.'
    Assert-Expected ($actual.startup_command -ceq ('"' + (Join-Path $install 'Prismestra.exe') + '" --installed --autostart')) 'Standard-user HKCU Run verification failed.'
    Assert-Expected (!$WebViewProbe -or $actual.native_webview_probe) 'Native original-user WebView2 probe was not verified.'
    Assert-Expected (!$WebViewProbe -or $actual.native_bootstrap) 'Native pre-runtime SID and startup commands were not verified.'
    Assert-Expected ($actual.seeded_ddp_devices -eq $DdpDevices -and (!$DdpDevices -or
        ($actual.first_scene.saved_map_stable -and $actual.first_scene.frames.Count -eq $DdpDevices))) 'Installed first-scene verification failed. See standard-user.json.'
} finally {
    try {
        if ($setupAttempted -and (Test-Path -LiteralPath $registry)) {
            $command = (Get-ItemProperty -LiteralPath $registry).QuietUninstallString
            Assert-Expected ($command -match '^"(.+\\unins\d{3}\.exe)"') 'Expected the production uninstaller.'
            $uninstaller = [IO.Path]::GetFullPath($Matches[1])
            Assert-Expected ([IO.Path]::GetDirectoryName($uninstaller) -ieq $install) 'Uninstaller escaped the production install directory.'
            Run-Setup $uninstaller '' (Join-Path $report 'uninstall.log')
            Assert-Expected (-not (Test-Path -LiteralPath (Join-Path $install 'Prismestra.exe'))) 'The production app was not removed.'
        }
    } finally {
        if ($account) {
            $sid = $account.SID.Value
            try {
                $until = [DateTime]::UtcNow.AddSeconds(15)
                do {
                    $profile = Get-CimInstance -ClassName Win32_UserProfile -Filter "SID='$sid'"
                    if (-not $profile -or -not $profile.Loaded) { break }
                    Start-Sleep -Milliseconds 500
                } while ([DateTime]::UtcNow -lt $until)
                if ($profile -and -not $profile.Loaded -and -not $profile.Special) { Remove-CimInstance -InputObject $profile }
                elseif ($profile) { Write-Warning 'The disposable fixture profile remains loaded.' }
            } finally { Remove-LocalUser -SID $account.SID }
        }
        if ($password) { $password.Dispose() }
        if (Test-Path -LiteralPath $probeDirectory) {
            [IO.File]::Delete((Join-Path $probeDirectory 'Prismestra-Setup-Probe.exe'))
            [IO.Directory]::Delete($probeDirectory)
        }
        if (Test-Path -LiteralPath $scratch) {
            foreach ($name in @('worker.ps1', 'invoke-worker.ps1', 'startup-helper.stderr.log', 'startup-helper.stdout.log', 'worker-output.log', 'app.stderr.log', 'app.stdout.log', 'native-current-sid.log', 'first_scene_receiver.cs')) {
                $file = Join-Path $scratch $name
                if (Test-Path -LiteralPath $file) { Remove-Item -LiteralPath $file }
            }
            [IO.Directory]::Delete($scratch)
        }
    }
}
