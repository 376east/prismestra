param(
    [Parameter(Mandatory = $true)][ValidateSet('Protect', 'Install', 'Uninstall')][string]$Action,
    [Parameter(Mandatory = $true)][string]$InstallRoot,
    [string]$UserSid
)

$ErrorActionPreference = 'Stop'
$PSModuleAutoLoadingPreference = 'None'
# Use the inbox module explicitly in this elevated process.
Import-Module ([IO.Path]::Combine($PSHOME, 'Modules', 'Microsoft.PowerShell.Utility', 'Microsoft.PowerShell.Utility.psd1'))

$system = 'S-1-5-18'
$administrators = 'S-1-5-32-544'
$users = 'S-1-5-32-545'
$installer = 'S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464'
$fileWrites = 0x500D0156
$sidType = [Security.Principal.SecurityIdentifier]
$utf8 = [Text.UTF8Encoding]::new($false, $true)

function New-NativeMethods {
    # In-memory P/Invoke definitions avoid emitting compiler inputs or assemblies in TEMP.
    $assemblyName = [Reflection.AssemblyName]::new('PrismestraInstallerNative')
    $assembly = [AppDomain]::CurrentDomain.DefineDynamicAssembly(
        $assemblyName, [Reflection.Emit.AssemblyBuilderAccess]::Run)
    $module = $assembly.DefineDynamicModule('NativeMethods')
    $type = $module.DefineType('PrismestraInstallerNative.Methods', 'Public, Abstract, Sealed')
    $methods = @(
        @{ Name = 'GetTokenInformation'; Dll = 'advapi32.dll'
           Params = [type[]]@([IntPtr], [int], [IntPtr], [int], [int].MakeByRefType()) },
        @{ Name = 'GetFileInformationByHandle'; Dll = 'kernel32.dll'
           Params = [type[]]@([IntPtr], [IntPtr]) },
        @{ Name = 'CloseHandle'; Dll = 'kernel32.dll'; Params = [type[]]@([IntPtr]) }
    )
    foreach ($entry in $methods) {
        $method = $type.DefinePInvokeMethod($entry.Name, $entry.Dll, 'Public, Static, PinvokeImpl',
            [Reflection.CallingConventions]::Standard, [int], $entry.Params,
            [Runtime.InteropServices.CallingConvention]::Winapi, [Runtime.InteropServices.CharSet]::Unicode)
        $method.SetImplementationFlags($method.GetMethodImplementationFlags() -bor
            [Reflection.MethodImplAttributes]::PreserveSig)
    }
    $type.CreateType()
}

function Read-TokenField([IntPtr]$Token, [int]$Field, [bool]$Pointer = $false) {
    $length = if ($Pointer) { [IntPtr]::Size } else { 4 }
    $memory = [Runtime.InteropServices.Marshal]::AllocHGlobal($length)
    try {
        $returned = 0
        if ($native::GetTokenInformation($Token, $Field, $memory, $length, [ref]$returned) -eq 0 -or
            $returned -ne $length) {
            throw 'Cannot validate the administrator token.'
        }
        if ($Pointer) { [Runtime.InteropServices.Marshal]::ReadIntPtr($memory) }
        else { [Runtime.InteropServices.Marshal]::ReadInt32($memory) }
    } finally { [Runtime.InteropServices.Marshal]::FreeHGlobal($memory) }
}

function Assert-LinkedLogin($Identity, [string]$Sid) {
    $session = Read-TokenField $Identity.Token 12
    if ($Identity.User.Value -ne $Sid -or $session -eq 0 -or
        (Read-TokenField $Identity.Token 18) -ne 2) {
        throw 'The elevated token must belong to the selected interactive administrator.'
    }
    $linked = Read-TokenField $Identity.Token 19 $true
    try {
        if ((Read-TokenField $linked 18) -ne 3 -or (Read-TokenField $linked 12) -ne $session) {
            throw 'The administrator has no corresponding limited interactive token.'
        }
        $normal = [Security.Principal.WindowsIdentity]::new($linked)
        try {
            $principal = [Security.Principal.WindowsPrincipal]::new($normal)
            if ($normal.User.Value -ne $Sid -or -not $principal.IsInRole($sidType::new('S-1-5-4'))) {
                throw 'The limited token belongs to a different interactive identity.'
            }
        } finally { $normal.Dispose() }
    } finally { [void]$native::CloseHandle($linked) }
}

function Get-Entry([string]$Path) {
    try { $attributes = [IO.File]::GetAttributes($Path) }
    catch [IO.FileNotFoundException] { return }
    catch [IO.DirectoryNotFoundException] { return }
    if ($attributes -band [IO.FileAttributes]::ReparsePoint) { throw "Reparse path: $Path" }
    if ($attributes -band [IO.FileAttributes]::Directory) { [IO.DirectoryInfo]::new($Path) }
    else { [IO.FileInfo]::new($Path) }
}

function Assert-NoHardLink([IO.FileInfo]$File) {
    $share = [IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete
    $handle = [IO.File]::Open($File.FullName, [IO.FileMode]::Open, [IO.FileAccess]::Read, $share)
    try {
        $memory = [Runtime.InteropServices.Marshal]::AllocHGlobal(52)
        try {
            if ($native::GetFileInformationByHandle($handle.SafeFileHandle.DangerousGetHandle(), $memory) -eq 0 -or
                [Runtime.InteropServices.Marshal]::ReadInt32($memory, 40) -ne 1) {
                throw "Hard link or unknown file identity: $($File.FullName)"
            }
        } finally { [Runtime.InteropServices.Marshal]::FreeHGlobal($memory) }
    } finally { $handle.Dispose() }
}

function Assert-Descriptor($Descriptor, $Trusted, [int]$WriteMask, [bool]$IgnoreInheritOnly) {
    if ($Descriptor.Owner -eq $null -or $Descriptor.Owner.Value -notin $Trusted -or
        $null -eq $Descriptor.DiscretionaryAcl) {
        throw 'Missing trusted owner or DACL.'
    }
    foreach ($ace in $Descriptor.DiscretionaryAcl) {
        if ($IgnoreInheritOnly -and ([int]$ace.AceFlags -band [int][Security.AccessControl.AceFlags]::InheritOnly)) {
            continue
        }
        if ($ace.AceType -eq [Security.AccessControl.AceType]::AccessDenied) { continue }
        if ($ace.AceType -ne [Security.AccessControl.AceType]::AccessAllowed -or
            (($ace.AccessMask -band $WriteMask) -and $ace.SecurityIdentifier.Value -notin $Trusted)) {
            throw 'Unsupported ACL entry or unprivileged write grant.'
        }
    }
}

function Assert-Protected($Entry, [bool]$SystemPath = $false, [int]$ExemptRights = 0,
    [bool]$ExistingInstall = $false) {
    if ($Entry -is [IO.DirectoryInfo]) { $acl = [IO.Directory]::GetAccessControl($Entry.FullName) }
    else { $acl = [IO.File]::GetAccessControl($Entry.FullName) }
    $descriptor = [Security.AccessControl.RawSecurityDescriptor]::new($acl.GetSecurityDescriptorBinaryForm(), 0)
    $trusted = @($system, $administrators)
    if ($SystemPath) { $trusted += $installer }
    $mask = $fileWrites -band (-bnot $ExemptRights)
    try { Assert-Descriptor $descriptor $trusted $mask ($SystemPath -or $ExistingInstall) }
    catch { throw "Unprotected path $($Entry.FullName): $_" }
    if ($Entry -is [IO.FileInfo]) { Assert-NoHardLink $Entry }
}

function Assert-Anchor([string]$Path, [bool]$ShareFinal) {
    $drive = [IO.DriveInfo]::new([IO.Path]::GetPathRoot($Path))
    if ($drive.DriveType -ne [IO.DriveType]::Fixed) { throw "Nonlocal Windows folder: $Path" }
    $ancestors = [Collections.Generic.Stack[IO.DirectoryInfo]]::new()
    $next = [IO.DirectoryInfo]::new($Path)
    while ($null -ne $next) { $ancestors.Push($next); $next = $next.Parent }
    while ($ancestors.Count) {
        $dir = $ancestors.Pop()
        $entry = Get-Entry $dir.FullName
        if ($entry -isnot [IO.DirectoryInfo]) { throw "Expected Windows directory: $($dir.FullName)" }
        $shared = $ShareFinal -and $ancestors.Count -eq 0
        $exempt = if ($shared) { 0x116 } elseif ($null -eq $dir.Parent) { 0x04 } else { 0 }
        Assert-Protected $entry $true $exempt
        if ($shared) {
            # The protected Windows child ensures the standard ProgramData
            # directory cannot become empty and be converted to a reparse point.
            $anchor = Get-Entry ([IO.Path]::Combine($dir.FullName, 'Microsoft'))
            if ($anchor -isnot [IO.DirectoryInfo]) { throw 'Missing protected Windows ProgramData anchor.' }
            Assert-Protected $anchor $true
        }
    }
}

function Get-ProtectedTree([IO.DirectoryInfo]$Root, [bool]$ExistingInstall) {
    $queue = [Collections.Generic.Queue[IO.DirectoryInfo]]::new()
    $queue.Enqueue($Root)
    $count = 0
    while ($queue.Count) {
        foreach ($child in $queue.Dequeue().EnumerateFileSystemInfos()) {
            if (++$count -gt 10000) { throw 'Protected directory contains too many entries.' }
            $entry = Get-Entry $child.FullName
            if ($null -eq $entry) { throw 'Protected directory changed during verification.' }
            Assert-Protected $entry $false $false $ExistingInstall
            $entry
            if ($entry -is [IO.DirectoryInfo]) { $queue.Enqueue($entry) }
        }
    }
}

function New-PrivateAcl([bool]$Directory, [string]$Reader) {
    if ($Directory) {
        $acl = [Security.AccessControl.DirectorySecurity]::new()
        $inherit = [Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit'
    } else {
        $acl = [Security.AccessControl.FileSecurity]::new()
        $inherit = [Security.AccessControl.InheritanceFlags]::None
    }
    $acl.SetOwner($sidType::new($administrators))
    $acl.SetAccessRuleProtection($true, $false)
    foreach ($account in @($system, $administrators, $Reader)) {
        $rights = if ($account -eq $Reader) { 'ReadAndExecute' } else { 'FullControl' }
        $acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new(
            $sidType::new($account), [Security.AccessControl.FileSystemRights]$rights,
            $inherit, [Security.AccessControl.PropagationFlags]::None,
            [Security.AccessControl.AccessControlType]::Allow))
    }
    $acl
}

function Set-Protected($Entry, [string]$Reader) {
    if ($Entry -is [IO.DirectoryInfo]) {
        [IO.Directory]::SetAccessControl($Entry.FullName, (New-PrivateAcl $true $Reader))
    } else {
        [IO.File]::SetAccessControl($Entry.FullName, (New-PrivateAcl $false $Reader))
    }
    Assert-Protected $Entry
}

function Protect-Application([string]$Root, $Entries) {
    Set-Protected ([IO.DirectoryInfo]::new($Root)) $users
    foreach ($entry in $Entries) { Set-Protected $entry $users }
}

function Get-ProtectedDirectory([string]$Path, [string]$Reader, [bool]$Create) {
    $entry = Get-Entry $Path
    if ($null -eq $entry -and $Create) {
        $entry = [IO.Directory]::CreateDirectory($Path, (New-PrivateAcl $true $Reader))
    }
    if ($null -ne $entry) {
        if ($entry -isnot [IO.DirectoryInfo]) { throw "Expected protected directory: $Path" }
        Assert-Protected $entry
        $entry
    }
}

function Read-BoundedJson([string]$Path, [long]$Limit) {
    $entry = Get-Entry $Path
    if ($entry -isnot [IO.FileInfo] -or $entry.Length -gt $Limit) { throw "Invalid JSON file: $Path" }
    Assert-Protected $entry $false $false $true
    [IO.File]::ReadAllText($Path, $utf8) | ConvertFrom-Json
}

function Write-ProtectedJson([string]$Directory, [string]$Name, $Value, [string]$Reader) {
    $path = [IO.Path]::Combine($Directory, $Name)
    $stage = [IO.Path]::Combine($Directory, [Guid]::NewGuid().ToString('N') + '.tmp')
    $bytes = $utf8.GetBytes((ConvertTo-Json -InputObject $Value -Depth 5 -Compress))
    try {
        $stream = [IO.File]::Open($stage, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write)
        try { $stream.Write($bytes, 0, $bytes.Length); $stream.Flush($true) } finally { $stream.Dispose() }
        Set-Protected ([IO.FileInfo]::new($stage)) $Reader
        if ([IO.File]::Exists($path)) { [IO.File]::Replace($stage, $path, [NullString]::Value) }
        else { [IO.File]::Move($stage, $path) }
    } finally { if ([IO.File]::Exists($stage)) { [IO.File]::Delete($stage) } }
}

function Get-Task($Folder, [string]$Name) {
    try { $Folder.GetTask($Name) }
    catch {
        $failure = $_.Exception.GetBaseException()
        if ($failure.HResult -ne -2147024894) { throw }
    }
}

function Assert-Task($Task, [string]$Sid, [string]$Image, [string]$Root) {
    $definition = $Task.Definition
    $principalSid = $definition.Principal.UserId
    if ($principalSid -notlike 'S-1-*') {
        $principalSid = [Security.Principal.NTAccount]::new($principalSid).Translate($sidType).Value
    }
    if ($definition.Actions.Count -ne 1 -or $definition.Triggers.Count -ne 0 -or
        $principalSid -ne $Sid -or $definition.Principal.RunLevel -ne 1 -or
        $definition.Principal.LogonType -ne 3) {
        throw 'Existing task belongs to a different authority.'
    }
    $entry = $definition.Actions.Item(1)
    if ($entry.Type -ne 0 -or $entry.Path -ne $Image -or $entry.WorkingDirectory -ne $Root -or
        $entry.Arguments -cne '--installed-backend-helper') {
        throw 'Existing task action belongs to a different installation.'
    }
    $descriptor = [Security.AccessControl.RawSecurityDescriptor]::new($Task.GetSecurityDescriptor(7))
    Assert-Descriptor $descriptor @($system, $administrators) $fileWrites $false
}

function Get-AppManifest([string]$Install, [string]$Executable) {
    $manifest = Read-BoundedJson ([IO.Path]::Combine($Install, 'install-manifest.json')) 262144
    $appHash = (Get-FileHash -LiteralPath $Executable -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($manifest.version -ne 1 -or $manifest.product -cne 'Prismestra' -or $manifest.app_sha256 -cne $appHash) {
        throw 'The installed application does not match its manifest.'
    }
    $manifest
}

function Get-RuntimePins([string]$Install, $Manifest) {
    $part = [IO.Path]::Combine($Install, 'components', 'openrgb')
    $component = Read-BoundedJson ([IO.Path]::Combine($part, 'component-manifest.json')) 2097152
    if ($component.version -ne 1 -or $component.scope -cne 'packaging_only' -or
        $component.engine_sha256 -cne $Manifest.backend.sha256) {
        throw 'The installed component does not match its manifest.'
    }
    $pins = [ordered]@{}
    foreach ($row in $component.files) {
        if ($row.path -cnotlike 'runtime/*') { continue }
        $name = $row.path.Substring(8)
        if ($name -cnotmatch '^[A-Za-z0-9_.+-]+$' -or $name.EndsWith('.') -or
            $row.sha256 -cnotmatch '^[a-f0-9]{64}$' -or $pins.Contains($name)) {
            throw 'Invalid or repeated runtime manifest pin.'
        }
        $pins.Add($name, $row.sha256)
    }
    if ($pins['OpenRGB.exe'] -cne $component.engine_sha256) { throw 'Missing OpenRGB image pin.' }
    $runtime = [IO.DirectoryInfo]::new([IO.Path]::Combine($part, 'runtime'))
    $entries = @(Get-ProtectedTree $runtime $true)
    if ($entries.Count -ne $pins.Count -or $entries.Count -gt 1024) { throw 'Runtime contents differ from the manifest.' }
    foreach ($entry in $entries) {
        if ($entry -isnot [IO.FileInfo] -or $entry.Name -cnotin @($pins.Keys) -or
            $entry.Length -gt 536870912 -or
            (Get-FileHash -LiteralPath $entry.FullName -Algorithm SHA256).Hash.ToLowerInvariant() -cne $pins[$entry.Name]) {
            throw "Installed runtime differs from the manifest: $($entry.Name)"
        }
    }
    $pins
}

try {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'Policy provisioning requires elevation.'
    }
    $native = New-NativeMethods
    if ($Action -ne 'Protect') {
        $sid = $sidType::new($UserSid).Value
        if ($sid -ne $UserSid -or $sid -notmatch '^S-1-(5-21|12-1)-[0-9-]+$') { throw 'Invalid account SID.' }
        if ($Action -eq 'Install') { Assert-LinkedLogin $identity $sid }
    }

    if ($InstallRoot -notmatch '^[a-zA-Z]:\\' -or $InstallRoot.Substring(2).Contains(':')) {
        throw 'The install directory must be an absolute local path.'
    }
    $root = [IO.Path]::GetFullPath($InstallRoot).TrimEnd('\')
    $programFiles = [Environment]::GetFolderPath([Environment+SpecialFolder]::ProgramFiles)
    Assert-Anchor $programFiles $false
    $prefix = $programFiles + '\'
    if (-not $root.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) {
        throw 'The install directory must be beneath Program Files.'
    }
    $directory = $programFiles
    foreach ($part in $root.Substring($prefix.Length).Split('\')) {
        $directory = [IO.Path]::Combine($directory, $part)
        $entry = Get-Entry $directory
        if ($entry -isnot [IO.DirectoryInfo]) { throw "Expected installed directory: $directory" }
        Assert-Protected $entry $false $false $true
    }
    $image = [IO.Path]::Combine($root, 'Prismestra.exe')
    $create = $Action -eq 'Install'
    if ($Action -ne 'Uninstall') {
        $entries = @(Get-ProtectedTree ([IO.DirectoryInfo]::new($root)) $true)
        if ($null -eq ($entries | Where-Object { $_ -is [IO.FileInfo] -and $_.FullName -eq $image })) {
            throw 'The installed application is missing.'
        }
        $manifest = Get-AppManifest $root $image
        $pins = Get-RuntimePins $root $manifest
        if ($Action -eq 'Protect') {
            Protect-Application $root $entries
            exit 0
        }
    }
    $programData = [Environment]::GetFolderPath([Environment+SpecialFolder]::CommonApplicationData)
    Assert-Anchor $programData $true
    $data = Get-ProtectedDirectory ([IO.Path]::Combine($programData, 'Prismestra')) $users $create
    if ($null -eq $data) { exit 0 }
    $policies = Get-ProtectedDirectory ([IO.Path]::Combine($data.FullName, 'Policies')) $users $create
    if ($null -eq $policies) { exit 0 }
    $policyHome = Get-ProtectedDirectory ([IO.Path]::Combine($policies.FullName, $sid)) $sid $create
    if ($null -eq $policyHome) { exit 0 }
    $null = @(Get-ProtectedTree $policyHome $false)
    $recordPath = [IO.Path]::Combine($policyHome.FullName, 'installation.json')
    $policyPath = [IO.Path]::Combine($policyHome.FullName, 'policy.json')
    $record = if ([IO.File]::Exists($recordPath)) { Read-BoundedJson $recordPath 4096 }
    if ($null -ne $record -and ($record.version -ne 1 -or $record.executable -ne $image -or $record.sid -ne $sid -or
        $record.app_sha256 -cnotmatch '^[a-f0-9]{64}$')) {
        throw 'The existing policy belongs to another installation.'
    }
    if ($null -eq $record -and [IO.File]::Exists($policyPath)) { throw 'The existing policy has no ownership record.' }
    $policy = if ([IO.File]::Exists($policyPath)) { Read-BoundedJson $policyPath 131072 }
    if ($null -ne $policy -and ($policy.version -ne 1 -or $policy.sid -ne $sid -or
        $policy.app_sha256 -cnotmatch '^[a-f0-9]{64}$' -or
        $policy.app_sha256 -cnotin @($record.app_sha256, $record.previous_app_sha256))) {
        throw 'The existing policy does not match its protected installation ownership record.'
    }

    $scheduler = New-Object -ComObject 'Schedule.Service'
    $scheduler.Connect()
    $folder = $scheduler.GetFolder('\')
    $name = "Prismestra-Backend-$sid"
    $task = Get-Task $folder $name
    if ($null -ne $task) {
        if ($null -eq $record) { throw 'The existing task has no installation ownership record.' }
        Assert-Task $task $sid $image $root
    }

    if (-not $create) {
        if ($null -eq $record) { exit 0 }
        if ($null -ne $task) {
            try { $task.Stop(0) }
            catch { if ($_.Exception.GetBaseException().HResult -ne -2147216629) { throw } }
            $folder.DeleteTask($name, 0)
        }
        # The exact SID directory and every descendant were verified above.
        [IO.Directory]::Delete($policyHome.FullName, $true)
        exit 0
    }

    Protect-Application $root $entries
    $null = Get-ProtectedDirectory ([IO.Path]::Combine($policyHome.FullName, 'config')) $sid $true
    $null = Get-ProtectedDirectory ([IO.Path]::Combine($policyHome.FullName, 'outbox')) $sid $true
    $definition = $scheduler.NewTask(0)
    $definition.RegistrationInfo.Description = 'Prismestra installed lighting backend'
    $definition.Principal.UserId = $sid
    $definition.Principal.LogonType = 3 # TASK_LOGON_INTERACTIVE_TOKEN
    $definition.Principal.RunLevel = 1 # TASK_RUNLEVEL_HIGHEST
    $definition.Settings.AllowDemandStart = $true
    $definition.Settings.ExecutionTimeLimit = 'PT0S'
    $definition.Settings.DisallowStartIfOnBatteries = $false
    $definition.Settings.StopIfGoingOnBatteries = $false
    $definition.Settings.MultipleInstances = 2 # TASK_INSTANCES_IGNORE_NEW
    $exec = $definition.Actions.Create(0)
    $exec.Path = $image
    $exec.WorkingDirectory = $root
    $exec.Arguments = '--installed-backend-helper'
    $taskAcl = "O:BAG:BAD:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGX;;;$sid)"
    # Create or update the verified object only. Do not add a principal ACE
    # automatically, and suppress registration-trigger execution.
    $flags = if ($null -eq $task) { 0x32 } else { 0x34 }
    $registered = $null
    try {
        # The previous pin makes the ownership record usable if setup is interrupted
        # before publishing the new policy, including uninstall after an upgrade.
        Write-ProtectedJson $policyHome.FullName 'installation.json' @{
            version = 1; sid = $sid; executable = $image
            app_sha256 = $manifest.app_sha256; previous_app_sha256 = $policy.app_sha256
        } $sid
        $registered = $folder.RegisterTaskDefinition($name, $definition, $flags, $sid, $null, 3, $taskAcl)
        Assert-Task $registered $sid $image $root
        Write-ProtectedJson $policyHome.FullName 'policy.json' @{
            version = 1; sid = $sid; endpoint = '127.0.0.1:6742'
            app_sha256 = $manifest.app_sha256; runtime_sha256 = $pins
        } $sid
    } catch {
        if ($null -eq $task -and $null -ne $registered) { $folder.DeleteTask($name, 0) }
        if ($null -eq $record -and [IO.File]::Exists($recordPath)) { [IO.File]::Delete($recordPath) }
        throw
    }
    exit 0
} catch {
    Write-Error -ErrorAction Continue $_
    exit 1
}
