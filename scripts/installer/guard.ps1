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
    if ($Action -eq 'Stop') {
        $image = Join-Path $root 'Prismestra.exe'
        if ([IO.File]::Exists($image)) {
            $result = Start-Process -FilePath $image -ArgumentList '--shutdown-installed' `
                -WorkingDirectory $root -WindowStyle Hidden -Wait -PassThru
            exit $result.ExitCode
        }
    } elseif ($Action -notin @('Check', 'Prepare')) { exit 1 }
    exit 0
} catch {
    Write-Error -ErrorAction Continue $_
    exit 1
}
