[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidatePattern('^S-1-(5-21|12-1)-[0-9-]+$')][string]$OriginalSid,
    [ValidateSet('Audit', 'Prepare', 'Disable')][string]$Action = 'Audit'
)

$ErrorActionPreference = 'Stop'
$env:PSModulePath = [IO.Path]::Combine($PSHOME, 'Modules')
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
$utf8 = [Text.UTF8Encoding]::new($false, $true)
$sidType = [Security.Principal.SecurityIdentifier]
$system = 'S-1-5-18'
$admins = 'S-1-5-32-544'
$trustedInstaller = 'S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464'
$launchPin = '82def8e5478fa616d691ebda66d0277c8950cc1d092b0797f8e99218ce670318'
$systemShell = [IO.Path]::Combine([Environment]::SystemDirectory, 'WindowsPowerShell', 'v1.0', 'powershell.exe')

function New-Native {
    # In-memory imports only. The elevated process never compiles from user TEMP.
    $assembly = [AppDomain]::CurrentDomain.DefineDynamicAssembly(
        [Reflection.AssemblyName]::new('PrismestraLegacyRetirement'), [Reflection.Emit.AssemblyBuilderAccess]::Run)
    $type = $assembly.DefineDynamicModule('Native').DefineType('PrismestraLegacyRetirement.Native')
    $attributes = [Reflection.MethodAttributes]'Public, Static, PinvokeImpl'
    $intRef = [int].MakeByRefType()
    $imports = @(
        @{name='GetTokenInformation'; dll='advapi32.dll'; returns=[int]; args=@([IntPtr],[int],[IntPtr],[int],$intRef)},
        @{name='CloseHandle'; dll='kernel32.dll'; returns=[int]; args=@([IntPtr])},
        @{name='LocalFree'; dll='kernel32.dll'; returns=[IntPtr]; args=@([IntPtr])},
        @{name='CommandLineToArgvW'; dll='shell32.dll'; returns=[IntPtr]; args=@([string],$intRef)}
    )
    foreach ($import in $imports) {
        $method = $type.DefinePInvokeMethod($import.name, $import.dll, $attributes,
            [Reflection.CallingConventions]::Standard, $import.returns, [type[]]$import.args,
            [Runtime.InteropServices.CallingConvention]::Winapi, [Runtime.InteropServices.CharSet]::Unicode)
        $method.SetImplementationFlags([Reflection.MethodImplAttributes]::PreserveSig)
    }
    $type.CreateType()
}

function Get-LinkedToken([IntPtr]$Token) {
    $size = [IntPtr]::Size
    $field = [Runtime.InteropServices.Marshal]::AllocHGlobal($size)
    try {
        $returned = 0
        if ($native::GetTokenInformation($Token, 19, $field, $size, [ref]$returned) -eq 0 -or $returned -ne $size) {
            throw 'The original account has no verified limited token.'
        }
        $linked = [Runtime.InteropServices.Marshal]::ReadIntPtr($field)
        if ($linked -eq [IntPtr]::Zero) { throw 'The original account has no linked token.' }
        $linked
    } finally { [Runtime.InteropServices.Marshal]::FreeHGlobal($field) }
}

function Get-Arguments([string]$Command) {
    if ([string]::IsNullOrWhiteSpace($Command) -or $Command.Length -ge 32767 -or $Command.Contains([char]0)) {
        throw 'Invalid task command line.'
    }
    $count = 0
    $argv = $native::CommandLineToArgvW($Command, [ref]$count)
    if ($argv -eq [IntPtr]::Zero) { throw 'Cannot parse task command line.' }
    try {
        for ($index = 0; $index -lt $count; $index++) {
            $pointer = [Runtime.InteropServices.Marshal]::ReadIntPtr($argv, $index * [IntPtr]::Size)
            [Runtime.InteropServices.Marshal]::PtrToStringUni($pointer)
        }
    } finally { [void]$native::LocalFree($argv) }
}

function Get-LocalPath([string]$Path) {
    if ($Path.StartsWith('\\?\', [StringComparison]::Ordinal)) { $Path = $Path.Substring(4) }
    if ($Path -notmatch '^[A-Za-z]:\\' -or $Path.Substring(2).Contains(':')) { throw 'Expected a local path.' }
    $canonical = [IO.Path]::GetFullPath($Path)
    $drive = [IO.Path]::GetPathRoot($canonical)
    if ([IO.DriveInfo]::new($drive).DriveType -ne [IO.DriveType]::Fixed) { throw 'Expected a fixed local disk.' }
    $entry = $drive
    foreach ($part in $canonical.Substring($drive.Length).TrimEnd('\').Split('\')) {
        if ($part) { $entry = [IO.Path]::Combine($entry, $part) }
        if ([IO.File]::GetAttributes($entry) -band [IO.FileAttributes]::ReparsePoint) {
            throw 'A legacy source path traverses a reparse point.'
        }
    }
    $canonical.TrimEnd('\')
}

function Read-Json([string]$File, [long]$Maximum) {
    $fileInfo = [IO.FileInfo]::new((Get-LocalPath $File))
    if ($fileInfo.Length -gt $Maximum) { throw 'Legacy metadata exceeds the size limit.' }
    ConvertFrom-Json -InputObject ([IO.File]::ReadAllText($fileInfo.FullName, $utf8))
}

function Read-Sha([string]$File, [long]$Maximum) {
    $fileInfo = [IO.FileInfo]::new((Get-LocalPath $File))
    if ($fileInfo.Length -gt $Maximum) { throw 'Legacy file exceeds the size limit.' }
    (Get-FileHash -LiteralPath $fileInfo.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Assert-LegacyProfile([string]$DataRoot, [string]$ProjectPath, [string]$StatePath) {
    $projectFile = Get-LocalPath $ProjectPath
    $stateDirectory = Get-LocalPath $StatePath
    if ($projectFile -ne [IO.Path]::Combine($DataRoot, 'profile.json') -or
        $stateDirectory -ne [IO.Path]::Combine($DataRoot, 'state') -or
        -not [IO.Directory]::Exists($stateDirectory)) {
        throw 'Legacy task names a different profile or state directory.'
    }
    $local = Read-Json ([IO.Path]::Combine($stateDirectory, 'lighting-local.json')) 4194304
    $project = Read-Json $projectFile 4194304
    if ($local.version -ne 1 -or $project.version -ne 1 -or ($null -eq $project.setup -and $null -eq $project.lighting.setup)) {
        throw 'Legacy profile or local bindings do not match the product.'
    }
}

function Assert-Release([string]$Image) {
    $imageFile = Get-LocalPath $Image
    if ([IO.Path]::GetFileName($imageFile) -notin @('Prismestra.exe','pc-control-ui.exe')) {
        throw 'Unexpected legacy product executable.'
    }
    $manifest = Read-Json ([IO.Path]::Combine([IO.Path]::GetDirectoryName($imageFile), 'release-manifest.json')) 131072
    if ($manifest.product -cne 'Prismestra' -or $manifest.executable_sha256 -cnotmatch '^[a-f0-9]{64}$' -or
        (Read-Sha $imageFile 536870912) -cne $manifest.executable_sha256) {
        throw 'Legacy executable does not match its release manifest.'
    }
    $manifest
}

function Get-PinnedSource([string]$LauncherPath) {
    $launcher = Get-LocalPath $LauncherPath
    if ([IO.Path]::GetFileName($launcher) -ine 'Start Prismestra.ps1' -or
        (Read-Sha $launcher 131072) -cne $launchPin) { return }
    $root = [IO.Path]::GetDirectoryName($launcher)
    $pointer = Read-Json ([IO.Path]::Combine($root, 'live-release.json')) 4096
    if ($pointer.executable -cnotmatch '^versions/([a-f0-9]{7,40})/Prismestra/Prismestra\.exe$' -or
        $pointer.commit -cnotmatch '^[a-f0-9]{40}$' -or $pointer.sha256 -cnotmatch '^[a-f0-9]{64}$') { return }
    if (-not $pointer.commit.StartsWith($pointer.executable.Split('/')[1], [StringComparison]::Ordinal)) { return }
    $executable = Get-LocalPath ([IO.Path]::Combine($root, $pointer.executable))
    $release = Assert-Release $executable
    if ($release.executable_sha256 -cne $pointer.sha256 -or $release.commit -cne $pointer.commit) { return }
    $projectPath = Get-LocalPath ([IO.Path]::Combine($root, 'profile.json'))
    $statePath = Get-LocalPath ([IO.Path]::Combine($root, 'state'))
    Assert-LegacyProfile $root $projectPath $statePath
    [ordered]@{version=1; launcher=$launcher; executable=$executable; profile=$projectPath; state=$statePath}
}

function Get-LaunchSource([string]$Executable, [string[]]$Arguments, [int]$RunLevel, [string]$WorkingDirectory) {
    if ((Get-LocalPath $Executable) -ne $systemShell -or $Arguments.Count -ne 9) { return }
    $expected = @('-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-WindowStyle','Hidden','-File')
    for ($index = 0; $index -lt $expected.Count; $index++) {
        if ($Arguments[$index] -ine $expected[$index]) { return }
    }
    if (($Arguments[8] -ine '-Autostart' -or $RunLevel -ne 0) -and
        ($Arguments[8] -ine '-BackendOnly' -or $RunLevel -ne 1)) { return }
    $hint = Get-PinnedSource $Arguments[7]
    if ($null -eq $hint) { return }
    $root = [IO.Path]::GetDirectoryName($hint.launcher)
    if ((Get-LocalPath $WorkingDirectory) -eq $root) { $root }
}

function Write-SourceHint([string]$Source) {
    $context = $normal.Impersonate()
    try {
        $hint = Get-PinnedSource ([IO.Path]::Combine($Source, 'Start Prismestra.ps1'))
        if ($null -eq $hint) { throw 'The legacy source changed before preparation.' }
        $data = Get-LocalPath ([Environment]::GetFolderPath('LocalApplicationData'))
        $root = [IO.Path]::Combine($data, 'Prismestra')
        if (-not [IO.Directory]::Exists($root)) { $null = [IO.Directory]::CreateDirectory($root) }
        $root = Get-LocalPath $root
        $target = [IO.Path]::Combine($root, 'legacy-source.json')
        if ([IO.File]::Exists($target)) {
            $saved = Read-Json $target 16384
            foreach ($name in @('version','launcher','executable','profile','state')) {
                if ($saved.$name -cne $hint[$name]) { throw 'An existing migration hint belongs to another source.' }
            }
            return
        }
        $stage = [IO.Path]::Combine($root, 'legacy-source-' + [Guid]::NewGuid().ToString('N') + '.tmp')
        $bytes = $utf8.GetBytes((ConvertTo-Json -InputObject $hint -Compress))
        if ($bytes.Length -gt 16384) { throw 'Legacy source hint exceeds its limit.' }
        try {
            $file = [IO.File]::Open($stage, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write)
            try { $file.Write($bytes, 0, $bytes.Length); $file.Flush($true) } finally { $file.Dispose() }
            [IO.File]::Move($stage, $target)
        } finally { if ([IO.File]::Exists($stage)) { [IO.File]::Delete($stage) } }
    } finally { $context.Undo(); $context.Dispose() }
}

function Test-ImportedSource([string]$Source) {
    $context = $normal.Impersonate()
    try {
        $root = Get-LocalPath ([IO.Path]::Combine([Environment]::GetFolderPath('LocalApplicationData'), 'Prismestra'))
        $receipt = Read-Json ([IO.Path]::Combine($root, 'installed-migration.json')) 131072
        if ($receipt.version -ne 1 -or $receipt.complete -isnot [bool] -or -not $receipt.complete -or
            (Get-LocalPath $receipt.source.profile) -ne [IO.Path]::Combine($Source,'profile.json') -or
            (Get-LocalPath $receipt.source.state) -ne [IO.Path]::Combine($Source,'state')) { return $false }
        $docHash = $receipt.after.'profile.json'
        $localHash = $receipt.after.'state/lighting-local.json'
        ($docHash -cmatch '^[a-f0-9]{64}$' -and $localHash -cmatch '^[a-f0-9]{64}$' -and
            (Read-Sha ([IO.Path]::Combine($root,'profile.json')) 4194304) -ceq $docHash -and
            (Read-Sha ([IO.Path]::Combine($root,'state','lighting-local.json')) 4194304) -ceq $localHash)
    } catch { $false } finally { $context.Undo(); $context.Dispose() }
}

function Test-User([string]$Id) {
    if (-not $Id) { return $false }
    try {
        if ($Id -like 'S-1-*') { return $sidType::new($Id).Value -ceq $OriginalSid }
        [Security.Principal.NTAccount]::new($Id).Translate($sidType).Value -ceq $OriginalSid
    } catch { $false }
}

function Get-TaskSource($Task) {
    $definition = $Task.Definition
    if ($definition.Actions.Count -ne 1 -or $definition.Triggers.Count -ne 1 -or
        $definition.Principal.LogonType -ne 3 -or -not (Test-User $definition.Principal.UserId)) { return }
    $trigger = $definition.Triggers.Item(1)
    $exec = $definition.Actions.Item(1)
    if ($exec.Type -ne 0 -or $trigger.Type -ne 9 -or -not $trigger.Enabled -or -not (Test-User $trigger.UserId)) {
        return
    }
    $context = $normal.Impersonate()
    try {
        $argv = @(Get-Arguments ('placeholder ' + $exec.Arguments))
        if ($argv.Length -lt 2) { return }
        Get-LaunchSource $exec.Path $argv[1..($argv.Length - 1)] $definition.Principal.RunLevel $exec.WorkingDirectory
    } catch { return } finally { $context.Undo(); $context.Dispose() }
}

function Get-TaskDefinitionWithoutEnabled($Task) {
    $doc = [xml]$Task.Xml
    $enabled = $doc.SelectSingleNode("/*[local-name()='Task']/*[local-name()='Settings']/*[local-name()='Enabled']")
    if ($null -ne $enabled) { [void]$enabled.ParentNode.RemoveChild($enabled) }
    $doc.OuterXml
}

function Assert-ProtectedDirectory([string]$Path, [string[]]$Owners, [int]$Exempt = 0, [bool]$SystemFolder = $false) {
    $canonical = Get-LocalPath $Path
    $acl = [IO.Directory]::GetAccessControl($canonical)
    $dacl = [Security.AccessControl.RawSecurityDescriptor]::new($acl.GetSecurityDescriptorBinaryForm(), 0)
    if ($dacl.Owner.Value -notin $Owners -or $null -eq $dacl.DiscretionaryAcl) { throw 'Untrusted rollback folder owner.' }
    foreach ($ace in $dacl.DiscretionaryAcl) {
        if ($SystemFolder -and ([int]$ace.AceFlags -band 8)) { continue }
        if ($ace.AceType -eq [Security.AccessControl.AceType]::AccessDenied) { continue }
        if ($ace.AceType -ne [Security.AccessControl.AceType]::AccessAllowed -or
            (($ace.AccessMask -band (0x500D0156 -band (-bnot $Exempt))) -and $ace.SecurityIdentifier.Value -notin $Owners)) {
            throw 'Rollback folder is writable by an unprivileged identity.'
        }
    }
}

function Save-Rollback($Records) {
    $base = [Environment]::GetFolderPath('CommonApplicationData')
    $owners = @($system,$admins)
    $drive = [IO.Path]::GetPathRoot($base)
    if ([IO.Path]::Combine($drive, 'ProgramData') -ne $base) { throw 'Unexpected ProgramData anchor.' }
    Assert-ProtectedDirectory $drive @($system,$admins,$trustedInstaller) 4 $true
    Assert-ProtectedDirectory $base $owners 0x116 $true
    Assert-ProtectedDirectory ([IO.Path]::Combine($base, 'Microsoft')) $owners
    $acl = [Security.AccessControl.DirectorySecurity]::new()
    $acl.SetOwner($sidType::new($admins))
    $acl.SetAccessRuleProtection($true, $false)
    $fileAcl = [Security.AccessControl.FileSecurity]::new()
    $fileAcl.SetOwner($sidType::new($admins))
    $fileAcl.SetAccessRuleProtection($true, $false)
    foreach ($owner in $owners) {
        $acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new(
            $sidType::new($owner), 'FullControl', 'ContainerInherit,ObjectInherit', 'None', 'Allow'))
        $fileAcl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new(
            $sidType::new($owner), 'FullControl', 'Allow'))
    }
    $directory = $base
    foreach ($part in @('Prismestra Legacy Startup Rollback',$OriginalSid)) {
        $directory = [IO.Path]::Combine($directory, $part)
        if (-not [IO.Directory]::Exists($directory)) { $null = [IO.Directory]::CreateDirectory($directory, $acl) }
        Assert-ProtectedDirectory $directory $owners
    }
    $name = [Guid]::NewGuid().ToString('N')
    $target = [IO.Path]::Combine($directory, "$name.json")
    $stage = [IO.Path]::Combine($directory, "$name.tmp")
    $bytes = $utf8.GetBytes((ConvertTo-Json -InputObject @{
        version=1; original_sid=$OriginalSid; tasks=$Records
    } -Depth 6 -Compress))
    if ($bytes.Length -gt 1048576) { throw 'Rollback data exceeds its limit.' }
    try {
        $file = [IO.File]::Open($stage, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write)
        try { $file.Write($bytes, 0, $bytes.Length); $file.Flush($true) } finally { $file.Dispose() }
        [IO.File]::SetAccessControl($stage, $fileAcl)
        [IO.File]::Move($stage, $target)
    } finally { if ([IO.File]::Exists($stage)) { [IO.File]::Delete($stage) } }
    $target
}

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]::new($identity)
if ($identity.IsSystem -or $identity.User.Value -cne $OriginalSid -or
    -not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    @{status='unchanged_original_split_token_required'} | ConvertTo-Json -Compress
    exit 0
}
$native = New-Native
try { $linked = Get-LinkedToken $identity.Token }
catch {
    @{status='unchanged_original_split_token_required'} | ConvertTo-Json -Compress
    exit 0
}
try {
    $normal = [Security.Principal.WindowsIdentity]::new($linked)
    try {
        $normalPrincipal = [Security.Principal.WindowsPrincipal]::new($normal)
        if ($normal.User.Value -cne $OriginalSid -or $normalPrincipal.IsInRole(
                [Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'Linked token does not match the original user.' }
        $scheduler = New-Object -ComObject 'Schedule.Service'
        $scheduler.Connect()
        $folder = $scheduler.GetFolder('\')
        $active = [Collections.Generic.List[object]]::new()
        $unchanged = [Collections.Generic.List[string]]::new()
        $tasks = $folder.GetTasks(1)
        if ($tasks.Count -gt 512) { throw 'Root task folder exceeds verification limits.' }
        foreach ($task in $tasks) {
            if (-not $task.Enabled) { continue }
            $source = Get-TaskSource $task
            if ($source) {
                $xml = $task.Xml
                if ($xml.Length -gt 131072 -or $active.Count -ge 8) { throw 'Legacy task proof exceeds verification limits.' }
                $active.Add(@{name=$task.Name; source=$source; xml=$xml; security=$task.GetSecurityDescriptor(7); enabled=$true})
            } elseif ($task.Name -match 'Prismestra') { $unchanged.Add($task.Name) }
        }
        $run = [Collections.Generic.List[string]]::new()
        foreach ($subkey in @('Run','RunOnce')) {
            $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey("Software\Microsoft\Windows\CurrentVersion\$subkey")
            if ($null -ne $key) {
                try {
                    foreach ($name in $key.GetValueNames()) {
                        $command = $key.GetValue($name, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
                        if ($name -match 'Prismestra' -or ($command -is [string] -and $command -match 'Prismestra\.exe|Start Prismestra\.ps1')) {
                            $run.Add("$subkey\$name")
                        }
                    }
                } finally { $key.Dispose() }
            }
        }
        $report = @{status='verified'; verified=@($active | ForEach-Object {$_.name});
            unchanged_tasks=$unchanged.ToArray(); unchanged_run_entries=$run.ToArray(); disabled=@(); rollback=$null}
        $sources = @($active | ForEach-Object {$_.source} | Sort-Object -Unique)
        if ($sources.Count -gt 1) {
            $report.status = 'unchanged_ambiguous_sources'
            ConvertTo-Json -InputObject $report -Depth 5 -Compress
            exit 0
        }
        if ($Action -eq 'Prepare' -and $active.Count -gt 0) {
            Write-SourceHint $sources[0]
            $report.status = 'prepared'
        }
        if ($Action -eq 'Disable' -and $active.Count -gt 0) {
            if (-not (Test-ImportedSource $sources[0])) {
                $report.status = 'unchanged_import_not_confirmed'
                ConvertTo-Json -InputObject $report -Depth 5 -Compress
                exit 0
            }
            $report.rollback = Save-Rollback $active.ToArray()
            $disabled = [Collections.Generic.List[object]]::new()
            try {
                foreach ($record in $active) {
                    $task = $folder.GetTask($record.name)
                    if (-not $task.Enabled -or $task.Xml -cne $record.xml -or
                        $task.GetSecurityDescriptor(7) -cne $record.security -or
                        (Get-TaskSource $task) -ne $record.source) {
                        throw 'A verified legacy task changed before retirement.'
                    }
                    $definition = Get-TaskDefinitionWithoutEnabled $task
                    # Disable future triggers. Do not stop or terminate a running task.
                    $task.Enabled = $false
                    $disabled.Add(@{record=$record; definition=$definition})
                    if ($folder.GetTask($record.name).Enabled) { throw 'Legacy task remains enabled.' }
                }
            } catch {
                $failure = $_.Exception.Message
                $rollbackErrors = [Collections.Generic.List[string]]::new()
                for ($index = $disabled.Count - 1; $index -ge 0; $index--) {
                    $item = $disabled[$index]
                    try {
                        $current = $folder.GetTask($item.record.name)
                        if ($current.GetSecurityDescriptor(7) -cne $item.record.security -or
                            (Get-TaskDefinitionWithoutEnabled $current) -cne $item.definition) {
                            throw 'Definition or access control changed.'
                        }
                        if (-not $current.Enabled) { $current.Enabled = $true }
                        if (-not $folder.GetTask($item.record.name).Enabled) { throw 'Task remains disabled.' }
                    } catch { $rollbackErrors.Add("$($item.record.name) ($($_.Exception.Message))") }
                }
                if ($rollbackErrors.Count -gt 0) {
                    throw "$failure Rollback needs attention [$($rollbackErrors -join '; ')]. Receipt: $($report.rollback)"
                }
                throw "$failure Previously disabled tasks were restored. Receipt: $($report.rollback)"
            }
            $report.status = 'disabled'
            $report.disabled = @($disabled | ForEach-Object {$_.record.name})
        }
        ConvertTo-Json -InputObject $report -Depth 5 -Compress
    } finally { $normal.Dispose() }
} finally { [void]$native::CloseHandle($linked); $identity.Dispose() }
