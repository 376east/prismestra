param($Action, $InstallRoot, $ValueName, $ExpectedSid)
$ErrorActionPreference = 'Stop'

function New-TokenApi {
    # The installer embeds this script and creates no compiler or DLL files.
    $assembly = [AppDomain]::CurrentDomain.DefineDynamicAssembly(
        [Reflection.AssemblyName]::new('PrismestraSetupToken'), [Reflection.Emit.AssemblyBuilderAccess]::Run)
    $type = $assembly.DefineDynamicModule('Native').DefineType('PrismestraSetupToken.Native')
    $attrs = [Reflection.MethodAttributes]::Public -bor [Reflection.MethodAttributes]::Static -bor
        [Reflection.MethodAttributes]::PinvokeImpl
    $intRef = [int].MakeByRefType()
    $handleRef = [IntPtr].MakeByRefType()
    $imports = @(
        @{ Name = 'GetTokenInformation'; Library = 'advapi32.dll'; Params = @([IntPtr], [int], [IntPtr], [int], $intRef) },
        @{ Name = 'DuplicateToken'; Library = 'advapi32.dll'; Params = @([IntPtr], [int], $handleRef) },
        @{ Name = 'RegOpenCurrentUser'; Library = 'advapi32.dll'; Params = @([int], $handleRef) },
        @{ Name = 'CloseHandle'; Library = 'kernel32.dll'; Params = @([IntPtr]) }
    )
    foreach ($import in $imports) {
        $method = $type.DefinePInvokeMethod(
            $import.Name, $import.Library, $import.Name, $attrs, [Reflection.CallingConventions]::Standard,
            [int], [type[]]$import.Params, [Runtime.InteropServices.CallingConvention]::Winapi,
            [Runtime.InteropServices.CharSet]::Unicode)
        $method.SetImplementationFlags([Reflection.MethodImplAttributes]::PreserveSig)
    }
    $type.CreateType()
}

function Get-TokenFlag($Api, [IntPtr]$Token, [int]$Class, [IntPtr]$Buffer) {
    $written = 0
    if ($Api::GetTokenInformation($Token, $Class, $Buffer, 4, [ref]$written) -eq 0 -or $written -ne 4) {
        throw 'Could not verify the Windows token used by Setup.'
    }
    [Runtime.InteropServices.Marshal]::ReadInt32($Buffer)
}

function Test-WebViewVersion($Value) {
    if ($Value -isnot [string]) { return $false }
    $parsed = $null
    if (-not [version]::TryParse($Value, [ref]$parsed) -or $parsed.Revision -lt 0 -or
        $Value -cne $parsed.ToString(4)) { return $false }
    $parsed -ge [version]'105.0.0.0' -and $parsed.Major -le 65535 -and
        $parsed.Minor -le 65535 -and $parsed.Build -le 65535 -and $parsed.Revision -le 65535
}

$api = $identity = $effective = $impersonation = $rootHandle = $null
$linked = $duplicate = $buffer = [IntPtr]::Zero
try {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    if ($Action -eq 'Probe') {
        if ($identity.User.Value -eq $ExpectedSid) { exit 0 }
        exit 1
    }
    if ($ExpectedSid -and $identity.User.Value -cne $ExpectedSid) { exit 1 }
    if ($Action -notin @('Launch', 'PrepareProfile', 'WebView', 'EnableStartup', 'DisableStartup')) { exit 1 }

    $api = New-TokenApi
    $buffer = [Runtime.InteropServices.Marshal]::AllocHGlobal([IntPtr]::Size)
    $admin = [Security.Principal.WindowsBuiltInRole]::Administrator
    $elevated = (Get-TokenFlag $api $identity.Token 20 $buffer) -ne 0 -or
        [Security.Principal.WindowsPrincipal]::new($identity).IsInRole($admin)
    if ($Action -in @('Launch', 'PrepareProfile')) {
        if ($elevated) { exit 1 }
        $image = Join-Path $InstallRoot 'Prismestra.exe'
        if ($Action -eq 'PrepareProfile') {
            $prepared = Start-Process -FilePath $image -ArgumentList '--prepare-installed' -WorkingDirectory $InstallRoot -WindowStyle Hidden -Wait -PassThru
            exit $prepared.ExitCode
        }
        Start-Process -FilePath $image -ArgumentList '--installed' -WorkingDirectory $InstallRoot
        exit 0
    }

    if ($elevated) {
        $written = 0
        if ((Get-TokenFlag $api $identity.Token 18 $buffer) -ne 2 -or
            $api::GetTokenInformation($identity.Token, 19, $buffer, [IntPtr]::Size, [ref]$written) -eq 0) {
            throw 'Start Setup from the ordinary Windows account to configure per-user startup.'
        }
        $linked = [Runtime.InteropServices.Marshal]::ReadIntPtr($buffer)
        if ($written -ne [IntPtr]::Size -or $linked -eq [IntPtr]::Zero -or
            (Get-TokenFlag $api $linked 18 $buffer) -ne 3 -or
            (Get-TokenFlag $api $linked 20 $buffer) -ne 0 -or
            $api::DuplicateToken($linked, 2, [ref]$duplicate) -eq 0) {
            throw 'Setup could not obtain its linked limited Windows token.'
        }
        $impersonation = [Security.Principal.WindowsIdentity]::Impersonate($duplicate)
        $effective = [Security.Principal.WindowsIdentity]::GetCurrent($true)
        if (-not $effective -or $effective.User.Value -cne $identity.User.Value -or
            [Security.Principal.WindowsPrincipal]::new($effective).IsInRole($admin)) {
            throw 'Setup refused an unexpected user or elevated impersonation token.'
        }
        # The predefined HKCU handle may be cached from the elevated process.
        $hkey = [IntPtr]::Zero
        $access = if ($Action -eq 'WebView') { 0x20019 } else { 0x2001f }
        if ($api::RegOpenCurrentUser($access, [ref]$hkey) -ne 0) {
            throw 'Setup could not open the per-user registry with the limited token.'
        }
        $rootHandle = [Microsoft.Win32.SafeHandles.SafeRegistryHandle]::new($hkey, $true)
        $root = [Microsoft.Win32.RegistryKey]::FromHandle($rootHandle)
    } else {
        $root = [Microsoft.Win32.Registry]::CurrentUser
    }
    if ($Action -eq 'WebView') {
        $key = $root.OpenSubKey('Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}')
        if (-not $key) { exit 1 }
        try { $version = $key.GetValue('pv') }
        finally { $key.Dispose() }
        if (Test-WebViewVersion $version) { exit 0 }
        exit 1
    }
    $image = Join-Path $InstallRoot 'Prismestra.exe'
    $command = '"' + $image + '" --installed --autostart'
    $key = $root.CreateSubKey('Software\Microsoft\Windows\CurrentVersion\Run')
    try {
        $prior = $key.GetValue($ValueName, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        if ($Action -eq 'EnableStartup') {
            if ($null -ne $prior -and $prior -cne $command) { exit 2 }
            $key.SetValue($ValueName, $command, [Microsoft.Win32.RegistryValueKind]::String)
        } elseif ($prior -ceq $command) {
            $key.DeleteValue($ValueName, $false)
        }
    } finally { $key.Dispose() }
    exit 0
} catch {
    Write-Error -ErrorAction Continue $_
    exit 1
} finally {
    try {
        if ($rootHandle) { $rootHandle.Dispose() }
        if ($effective) { $effective.Dispose() }
    } finally {
        try {
            if ($impersonation) {
                try { $impersonation.Undo() }
                finally { $impersonation.Dispose() }
            }
        } finally {
            if ($duplicate -ne [IntPtr]::Zero) { [void]$api::CloseHandle($duplicate) }
            if ($linked -ne [IntPtr]::Zero) { [void]$api::CloseHandle($linked) }
            if ($buffer -ne [IntPtr]::Zero) { [Runtime.InteropServices.Marshal]::FreeHGlobal($buffer) }
            if ($identity) { $identity.Dispose() }
        }
    }
}
