param($Action, $InstallRoot)
$ErrorActionPreference = 'Stop'

# This code is embedded in Setup and Uninstall independently of installed files.
$trusted = @('S-1-5-18', 'S-1-5-32-544',
    'S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464')
$owner = [Security.Principal.SecurityIdentifier]
function Assert-Protected($item, [switch]$ProgramFilesAncestor) {
    if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -or $item.LinkType -eq 'HardLink') {
        throw 'The installation contains a reparse point or hard link.'
    }
    $acl = Get-Acl -LiteralPath $item.FullName
    if ($acl.GetOwner($owner).Value -notin $trusted) { throw 'An installation owner is untrusted.' }
    foreach ($ace in $acl.GetAccessRules($true, $true, $owner)) {
        $inheritOnly = $ace.PropagationFlags -band [Security.AccessControl.PropagationFlags]::InheritOnly
        # Program Files may define inherited defaults for unrelated products.
        if ($inheritOnly -and $ProgramFilesAncestor) { continue }
        if ($ace.AccessControlType -eq 'Allow' -and ($ace.FileSystemRights -band 0x500D0156) -and
            $ace.IdentityReference.Value -notin $trusted) {
            throw 'The installation is writable by an unprivileged identity.'
        }
    }
}

function New-ProtectedDirectory($path) {
    $permissions = [Security.AccessControl.DirectorySecurity]::new()
    $permissions.SetOwner([Security.Principal.SecurityIdentifier]::new('S-1-5-32-544'))
    $permissions.SetAccessRuleProtection($true, $false)
    $inheritance = [Security.AccessControl.InheritanceFlags]::ContainerInherit -bor
        [Security.AccessControl.InheritanceFlags]::ObjectInherit
    foreach ($sid in @('S-1-5-18', 'S-1-5-32-544', 'S-1-5-32-545')) {
        $rights = if ($sid -eq 'S-1-5-32-545') { 'ReadAndExecute' } else { 'FullControl' }
        $rule = [Security.AccessControl.FileSystemAccessRule]::new(
            [Security.Principal.SecurityIdentifier]::new($sid), $rights, $inheritance, 'None', 'Allow')
        $permissions.AddAccessRule($rule)
    }
    [IO.Directory]::CreateDirectory($path, $permissions)
}

function Retire-LegacyCrt($root, [switch]$Remove) {
    # Exact app-local files from the published Preview 6 component.
    $legacyCrt = @{
        'msvcp140.dll' = '7c26614e1d733892c2deac7e245ce115504b1d80592dd0a01b08e3e5a55f89ca'
        'msvcp140_codecvt_ids.dll' = '8a65c7596ef2e6938731f5a1058e7e40145b6d97967cc649231a076b9a608d78'
        'vcruntime140.dll' = 'd1f4225df2cd877dbf130d5668a021dce3f94118455ff5ec952061c30afc9ce7'
        'vcruntime140_1.dll' = 'a7146c08f89fe5b04541ab507cdb59ff7b44534d4ba3c668a426c6450a03434e'
    }
    $runtime = Join-Path $root 'components\openrgb\runtime'
    $ownedFiles = @(
        foreach ($name in $legacyCrt.Keys) {
            $file = Join-Path $runtime $name
            if (-not (Test-Path -LiteralPath $file)) { continue }
            if ((Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash -ine $legacyCrt[$name]) {
                throw "The private OpenRGB directory contains an unrecognized runtime file: $name"
            }
            $file
        }
    )
    if ($Remove) {
        foreach ($file in $ownedFiles) { [IO.File]::Delete($file) }
    }
}

try {
    $root = [IO.Path]::GetFullPath($InstallRoot)
    $programFiles = [Environment]::GetFolderPath([Environment+SpecialFolder]::ProgramFiles)
    $prefix = $programFiles + '\'
    if (-not $root.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) {
        throw 'The installation must be inside Program Files.'
    }
    $parent = Get-Item -LiteralPath $programFiles
    Assert-Protected $parent -ProgramFilesAncestor
    $path = $parent.FullName
    foreach ($part in $root.Substring($prefix.Length).Split('\')) {
        $path = Join-Path $path $part
        try { $entry = Get-Item -LiteralPath $path -Force }
        catch [System.Management.Automation.ItemNotFoundException] {
            if ($Action -ne 'Prepare') { exit 0 }
            $entry = New-ProtectedDirectory $path
        }
        if ($entry -isnot [IO.DirectoryInfo]) { throw 'Expected an installation directory.' }
        Assert-Protected $entry
    }
    $queue = [Collections.Generic.Queue[IO.DirectoryInfo]]::new()
    $queue.Enqueue($entry)
    $count = 0
    while ($queue.Count) {
        foreach ($child in $queue.Dequeue().EnumerateFileSystemInfos()) {
            if (++$count -gt 10000) { throw 'The installation tree has unexpected contents.' }
            Assert-Protected $child
            if ($child -is [IO.DirectoryInfo]) { $queue.Enqueue($child) }
        }
    }
    if ($Action -in @('Prepare', 'RemoveLegacyCrt')) {
        Retire-LegacyCrt $root -Remove:($Action -eq 'RemoveLegacyCrt')
    }
    if ($Action -eq 'Stop') {
        $image = Join-Path $root 'Prismestra.exe'
        if ([IO.File]::Exists($image)) {
            $result = Start-Process -FilePath $image -ArgumentList '--shutdown-installed' `
                -WorkingDirectory $root -WindowStyle Hidden -Wait -PassThru
            exit $result.ExitCode
        }
    } elseif ($Action -notin @('Check', 'Prepare', 'RemoveLegacyCrt')) { exit 1 }
    exit 0
} catch {
    Write-Error -ErrorAction Continue $_
    exit 1
}
