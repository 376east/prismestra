[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
if ($PSVersionTable.PSEdition -ne 'Desktop') {
    throw 'Run this check with inbox Windows PowerShell 5.1.'
}
$env:PSModulePath = [IO.Path]::Combine($PSHOME, 'Modules')
$utf8 = [Text.UTF8Encoding]::new($false, $true)
$scriptPath = Join-Path $PSScriptRoot 'retire-legacy-startup.ps1'
$tokens = $null
$parseErrors = $null
$tree = [Management.Automation.Language.Parser]::ParseFile($scriptPath, [ref]$tokens, [ref]$parseErrors)
if ($parseErrors) { throw ($parseErrors | Out-String) }

function Get-Definition([string]$Name) {
    $definition = $tree.Find({
        param($node)
        $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -ceq $Name
    }, $true)
    if ($null -eq $definition) { throw "Missing production function $Name." }
    $definition
}

function Get-Assignment([string]$Name) {
    $assignment = $tree.Find({
        param($node)
        $node -is [Management.Automation.Language.AssignmentStatementAst] -and
            $node.Left -is [Management.Automation.Language.VariableExpressionAst] -and
            $node.Left.VariablePath.UserPath -ceq $Name
    }, $true)
    if ($null -eq $assignment) { throw "Missing production assignment $Name." }
    $assignment
}

function Assert-Expected([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
}

# Load only helpers for path/argument verification. Never execute the production
# script body, obtain an elevated token, or connect to the real scheduler.
foreach ($name in @('New-Native', 'Get-Arguments', 'Get-LocalPath', 'Read-Json', 'Read-Sha',
    'Assert-LegacyProfile', 'Assert-Release', 'Get-PinnedSource', 'Get-LaunchSource', 'Get-TaskDefinitionWithoutEnabled')) {
    . ([scriptblock]::Create((Get-Definition $name).Extent.Text))
}
$productionPin = (Get-Assignment 'launchPin').Right.Extent.Text.Trim("'")
Assert-Expected ($productionPin -ceq '82def8e5478fa616d691ebda66d0277c8950cc1d092b0797f8e99218ce670318') (
    'The published legacy launcher pin differs from this fixture.')
$native = New-Native
$systemShell = [IO.Path]::Combine([Environment]::SystemDirectory, 'WindowsPowerShell', 'v1.0', 'powershell.exe')

function Set-JsonFixture([string]$Path, $Value) {
    [IO.File]::WriteAllText($Path, (ConvertTo-Json -InputObject $Value -Depth 6 -Compress), $utf8)
}

function Test-LauncherProof([string]$Root) {
    $dataRoot = [IO.Directory]::CreateDirectory((Join-Path $Root 'legacy')).FullName
    $state = [IO.Directory]::CreateDirectory((Join-Path $dataRoot 'state')).FullName
    $release = [IO.Directory]::CreateDirectory([IO.Path]::Combine($dataRoot, 'versions', 'aaaaaaa', 'Prismestra')).FullName
    $fixtureExe = Join-Path $release 'Prismestra.exe'
    [IO.File]::WriteAllText($fixtureExe, 'Synthetic executable bytes. Never executed.', $utf8)
    $appPin = Read-Sha $fixtureExe 4096
    $commit = 'a' * 40
    Set-JsonFixture (Join-Path $release 'release-manifest.json') @{product='Prismestra'; commit=$commit; executable_sha256=$appPin}
    Set-JsonFixture (Join-Path $dataRoot 'live-release.json') @{
        executable='versions/aaaaaaa/Prismestra/Prismestra.exe'; sha256=$appPin; commit=$commit
    }
    Set-JsonFixture (Join-Path $dataRoot 'profile.json') @{version=1; lighting=@{setup=@{}}}
    Set-JsonFixture (Join-Path $state 'lighting-local.json') @{version=1}
    $launcher = Join-Path $dataRoot 'Start Prismestra.ps1'
    [IO.File]::WriteAllText($launcher, '# Synthetic known launcher. Never executed.', $utf8)
    # Verify real comparison logic against an independent, temporary source tree.
    $launchPin = Read-Sha $launcher 4096
    $hint = Get-PinnedSource $launcher
    Assert-Expected ($hint.version -eq 1 -and $hint.launcher -ceq $launcher -and $hint.executable -ceq $fixtureExe -and
        $hint.profile -ceq (Join-Path $dataRoot 'profile.json') -and $hint.state -ceq $state) 'Unexpected migration hint.'
    $command = 'placeholder -NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -File "' + $launcher + '" '
    foreach ($role in @(@('-Autostart', 0), @('-BackendOnly', 1))) {
        $argv = @(Get-Arguments ($command + $role[0]))
        Assert-Expected ((Get-LaunchSource $systemShell $argv[1..9] $role[1] $dataRoot) -ceq $dataRoot) (
            'Verified legacy launcher was not recognized.')
        Assert-Expected (-not (Get-LaunchSource $systemShell $argv[1..9] (1 - $role[1]) $dataRoot)) (
            'Launcher with mismatched run level was accepted.')
        $argv[5] = '-NoExit'
        Assert-Expected (-not (Get-LaunchSource $systemShell $argv[1..9] $role[1] $dataRoot)) (
            'Changed PowerShell command line was accepted.')
    }
    $argv = @(Get-Arguments ($command + '-Autostart'))
    [IO.File]::AppendAllText($launcher, ' changed', $utf8)
    Assert-Expected (-not (Get-LaunchSource $systemShell $argv[1..9] 0 $dataRoot)) 'Unknown launcher bytes were accepted.'
    Assert-Expected (-not (Get-LaunchSource $fixtureExe @('--project','anything','--start-connected') 0 $dataRoot)) (
        'Self-pinned direct executable was accepted.')
}

function Test-IsolatedRollbackRoot {
    # Inspect the production directory walk without touching machine ProgramData.
    $definition = Get-Definition 'Save-Rollback'
    $loop = $definition.Find({
        param($node)
        $node -is [Management.Automation.Language.ForEachStatementAst] -and
            $node.Variable.VariablePath.UserPath -ceq 'part'
    }, $true)
    Assert-Expected ($null -ne $loop) 'Missing protected rollback directory walk.'
    Assert-Expected ($loop.Condition.Extent.Text -ceq "@('Prismestra Legacy Startup Rollback',`$OriginalSid)") (
        'The rollback hierarchy must be isolated from shared Prismestra policy files.')
    $source = $definition.Extent.Text
    Assert-Expected ($source.IndexOf('[IO.File]::SetAccessControl($stage, $fileAcl)') -ge 0 -and
        $source.IndexOf('[IO.File]::SetAccessControl($stage, $fileAcl)') -lt
        $source.IndexOf('[IO.File]::Move($stage, $target)')) 'Rollback bytes must be ACL-protected before publication.'
}

function Test-MigrationReceiptGate([string]$Root) {
    # Substitute only the known-folder lookup, and use a fake impersonation
    # context. The production receipt checks see only files in the fixture.
    $source = (Get-Definition 'Test-ImportedSource').Extent.Text
    $lookup = "[Environment]::GetFolderPath('LocalApplicationData')"
    Assert-Expected ($source.Contains($lookup)) 'Fixture cannot isolate the installed profile lookup.'
    . ([scriptblock]::Create($source.Replace($lookup, '$fakeLocalData')))
    $context = [pscustomobject]@{}
    $context | Add-Member ScriptMethod Undo {}
    $context | Add-Member ScriptMethod Dispose {}
    $normal = [pscustomobject]@{Context=$context}
    $normal | Add-Member ScriptMethod Impersonate {$this.Context}

    $fakeLocalData = [IO.Directory]::CreateDirectory((Join-Path $Root 'Local')).FullName
    $install = [IO.Directory]::CreateDirectory((Join-Path $fakeLocalData 'Prismestra')).FullName
    $state = [IO.Directory]::CreateDirectory((Join-Path $install 'state')).FullName
    $legacy = Join-Path $Root 'legacy'
    $receiptPath = Join-Path $install 'installed-migration.json'
    $installedProfile = Join-Path $install 'profile.json'
    $installedLocal = Join-Path $state 'lighting-local.json'
    Set-JsonFixture $installedProfile @{version=1}
    Set-JsonFixture $installedLocal @{version=1}
    Assert-Expected (-not (Test-ImportedSource $legacy)) 'A missing import receipt approved retirement.'

    $receipt = @{
        version=1; complete=$true
        source=@{profile=$installedProfile; state=(Join-Path $legacy 'state')}
        after=@{'profile.json'=(Read-Sha $installedProfile 4096); 'state/lighting-local.json'=(Read-Sha $installedLocal 4096)}
    }
    Set-JsonFixture $receiptPath $receipt
    Assert-Expected (-not (Test-ImportedSource $legacy)) 'A receipt for another profile approved retirement.'
    $receipt.source.profile = Join-Path $legacy 'profile.json'
    $receipt.complete = $false
    Set-JsonFixture $receiptPath $receipt
    Assert-Expected (-not (Test-ImportedSource $legacy)) 'An incomplete import approved retirement.'
    $receipt.complete = $true
    Set-JsonFixture $receiptPath $receipt
    Assert-Expected (Test-ImportedSource $legacy) 'Matching committed source and installed bytes did not approve retirement.'
    [IO.File]::AppendAllText($installedLocal, ' ', $utf8)
    Assert-Expected (-not (Test-ImportedSource $legacy)) 'Changed installed bindings approved retirement.'
}

function New-MockTask([string]$Name, [string]$Source, [bool]$BreakRollback = $false) {
    $task = [pscustomobject]@{
        Name=$Name; Source=$Source; Enabled=$true; BreakRollback=$BreakRollback
        Xml='<Task><Settings><Enabled>true</Enabled></Settings></Task>'
    }
    $task | Add-Member ScriptMethod GetSecurityDescriptor {
        param($Flags)
        if ($this.BreakRollback -and -not $this.Enabled) { 'changed' } else { 'verified' }
    }
    $task
}

function Add-MockRecord($Records, $Task) {
    $Records.Add(@{name=$Task.Name; source=$Task.Source; xml=$Task.Xml; security='verified'; enabled=$true})
}

function Test-RetirementTransaction {
    # Execute only the reporting/transaction statements on snapshots and
    # fake task objects. Refuse to run if new commands enter the selected block.
    $reportStart = Get-Assignment 'report'
    $statements = @($reportStart.Parent.Statements | Where-Object {
        $_.Extent.StartOffset -ge $reportStart.Extent.StartOffset
    })
    Assert-Expected ($statements.Count -eq 6) 'Unexpected retirement transaction shape.'
    $commands = @('ForEach-Object', 'Sort-Object', 'ConvertTo-Json', 'Save-Rollback',
        'Get-TaskSource', 'Get-TaskDefinitionWithoutEnabled', 'Write-SourceHint', 'Test-ImportedSource')
    foreach ($statement in $statements) {
        $calls = $statement.FindAll({
            param($node) $node -is [Management.Automation.Language.CommandAst]
        }, $true)
        foreach ($call in $calls) {
            Assert-Expected ($call.GetCommandName() -cin $commands) 'Unexpected command in synthetic retirement.'
        }
    }
    $body = [scriptblock]::Create((($statements | ForEach-Object {$_.Extent.Text}) -join "`n").Replace('exit 0','return'))
    $Action = 'Disable'
    $active = [Collections.Generic.List[object]]::new()
    $unchanged = [Collections.Generic.List[string]]::new()
    $run = [Collections.Generic.List[string]]::new()
    $preimages = [Collections.Generic.List[object]]::new()
    $hints = [Collections.Generic.List[string]]::new()
    $receiptMatched = $false
    $one = New-MockTask 'legacy-ui' 'C:\legacy-a'
    $two = New-MockTask 'legacy-backend' 'C:\legacy-b'
    $folder = [pscustomobject]@{ByName=@{'legacy-ui'=$one; 'legacy-backend'=$two}}
    $folder | Add-Member ScriptMethod GetTask { param($Name) $this.ByName[$Name] }
    function Get-TaskSource($Task) { $Task.Source }
    function Write-SourceHint([string]$Source) { $hints.Add($Source) }
    function Test-ImportedSource([string]$Source) { $receiptMatched -and $Source -ceq $one.Source }
    function Save-Rollback($Records) {
        foreach ($record in $Records) {
            Assert-Expected ($folder.ByName[$record.name].Enabled) 'Task already disabled before its backup.'
            $preimages.Add($record)
        }
        'protected-fixture-receipt.json'
    }

    foreach ($task in @($one,$two)) { Add-MockRecord $active $task }
    $ambiguous = (& $body) | ConvertFrom-Json
    Assert-Expected ($ambiguous.status -ceq 'unchanged_ambiguous_sources' -and
        $one.Enabled -and $two.Enabled -and $preimages.Count -eq 0) 'Ambiguous sources were modified.'

    $two.Source = $one.Source
    $active.Clear()
    foreach ($task in @($one,$two)) { Add-MockRecord $active $task }
    $Action = 'Prepare'
    $prepared = (& $body) | ConvertFrom-Json
    Assert-Expected ($prepared.status -ceq 'prepared' -and $one.Enabled -and $two.Enabled -and
        $hints.Count -eq 1 -and $hints[0] -ceq $one.Source -and $preimages.Count -eq 0) (
        'Prepare must publish one source hint without disabling or backing up tasks.')
    $Action = 'Disable'
    $unimported = (& $body) | ConvertFrom-Json
    Assert-Expected ($unimported.status -ceq 'unchanged_import_not_confirmed' -and
        $one.Enabled -and $two.Enabled -and $preimages.Count -eq 0) 'Tasks changed without a matching completed import.'
    $receiptMatched = $true
    $done = (& $body) | ConvertFrom-Json
    Assert-Expected ($done.status -ceq 'disabled' -and -not $one.Enabled -and -not $two.Enabled -and
        $preimages.Count -eq 2 -and $preimages[0].xml -ceq $one.Xml -and $preimages[0].security -ceq 'verified') (
        'Both tasks should be disabled only after backing up their XML and ACL.')

    $preimages.Clear()
    $one.Enabled = $true
    $two.Enabled = $true
    $two.BreakRollback = $true
    $three = New-MockTask 'changed-before-disable' $one.Source
    $folder.ByName[$three.Name] = $three
    $active.Clear()
    foreach ($task in @($one,$two,$three)) { Add-MockRecord $active $task }
    $three.Xml = '<Task><Settings/><Changed/></Task>'
    $failure = ''
    try { $null = & $body } catch { $failure = $_.Exception.Message }
    Assert-Expected ($one.Enabled -and -not $two.Enabled -and $three.Enabled -and
        $failure -match 'legacy-backend.*protected-fixture-receipt') 'Rollback did not continue past the changed task.'
}

$temporaryRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\')
$fixtureName = 'prismestra-startup-fixture-' + [Guid]::NewGuid().ToString('N')
$fixture = [IO.Path]::Combine($temporaryRoot, $fixtureName)
$null = [IO.Directory]::CreateDirectory($fixture)
try {
    Test-LauncherProof $fixture
    Test-IsolatedRollbackRoot
    Test-MigrationReceiptGate $fixture
    Test-RetirementTransaction
} finally {
    $resolved = (Resolve-Path -LiteralPath $fixture).ProviderPath
    if ([IO.Path]::GetDirectoryName($resolved) -ne $temporaryRoot -or [IO.Path]::GetFileName($resolved) -cne $fixtureName -or
        ([IO.File]::GetAttributes($resolved) -band [IO.FileAttributes]::ReparsePoint)) {
        throw 'Refusing to clean up an unexpected fixture directory.'
    }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
'Legacy startup parser, source proof, isolated rollback root, ambiguity and rollback fixtures passed.'
