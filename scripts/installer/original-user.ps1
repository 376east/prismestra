param($Action, $InstallRoot, $ValueName, $ExpectedSid)
$ErrorActionPreference = 'Stop'
try {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    if ($principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { exit 1 }
    if ($Action -eq 'Probe') {
        if ($identity.User.Value -eq $ExpectedSid) { exit 0 }
        exit 1
    }
    if ($Action -eq 'WebView') {
        $key = 'HKCU:\Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
        $version = Get-ItemPropertyValue -LiteralPath $key -Name pv -ErrorAction SilentlyContinue
        if ($version -and [version]$version -gt [version]'0.0.0.0') { exit 0 }
        exit 1
    }
    $image = Join-Path $InstallRoot 'Prismestra.exe'
    if ($Action -eq 'Launch') {
        Start-Process -FilePath $image -ArgumentList '--installed' -WorkingDirectory $InstallRoot
        exit 0
    }
    if ($Action -notin @('EnableStartup', 'DisableStartup')) { exit 1 }
    $command = '"' + $image + '" --installed --autostart'
    $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Software\Microsoft\Windows\CurrentVersion\Run')
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
}
