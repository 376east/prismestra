param(
    [Parameter(Mandatory=$true)][string]$SourceRoot,
    [Parameter(Mandatory=$true)][string]$BuildRoot,
    [Parameter(Mandatory=$true)][string]$QtRoot,
    [Parameter(Mandatory=$true)][string]$Jom,
    [Parameter(Mandatory=$true)][string]$VsDevCmd,
    [ValidateRange(1,8)][int]$Jobs = 4
)
$ErrorActionPreference = 'Stop'
$sourcePath = (Resolve-Path -LiteralPath $SourceRoot).ProviderPath
$qtPath = (Resolve-Path -LiteralPath $QtRoot).ProviderPath
$jomPath = (Resolve-Path -LiteralPath $Jom).ProviderPath
$vsPath = (Resolve-Path -LiteralPath $VsDevCmd).ProviderPath
$gitDirectory = Split-Path -Parent (Get-Command git -CommandType Application | Select-Object -First 1).Source
if ((& git -C $sourcePath rev-parse HEAD) -ne '6bf7d9f5348be41dcf6a45dc7ad57f00b0b69b43') {
    throw 'Unexpected component source revision'
}
New-Item -ItemType Directory -Path $BuildRoot -Force | Out-Null
$buildPath = (Resolve-Path -LiteralPath $BuildRoot).ProviderPath
if ($buildPath -eq $sourcePath -or $buildPath.StartsWith($sourcePath + '\', [StringComparison]::OrdinalIgnoreCase)) {
    throw 'BuildRoot must be outside the component source tree'
}
# qmake stores its own executable path in Makefile recipes. Short ASCII paths
# are required for recursive calls too, not just for the initial invocation.
$free = @('Z:','Y:','X:','W:','V:','U:') | Where-Object { -not (Test-Path ($_.ToString() + '/')) } | Select-Object -First 3
if ($free.Count -ne 3) { throw 'Three temporary unused drive letters are required' }
$mappings = @()
$pushed = $false
$buildLock = $null
try {
    $buildLock = [IO.File]::Open((Join-Path $buildPath '.prismestra-build.lock'), 'OpenOrCreate', 'ReadWrite', 'None')
    # Keep source/work paths below drive roots for stable relative recipes.
    $targets = @((Split-Path -Parent $sourcePath), (Split-Path -Parent $buildPath), (Split-Path -Parent $qtPath))
    for ($i = 0; $i -lt 3; $i++) {
        & subst.exe $free[$i] $targets[$i]
        if ($LASTEXITCODE -ne 0) { throw 'Temporary build mapping failed' }
        $mappings += $free[$i]
    }
    $sourceDrive, $buildDrive, $qtDrive = $free
    $mappedSource = $sourceDrive + '/' + (Split-Path -Leaf $sourcePath)
    $mappedBuild = $buildDrive + '/' + (Split-Path -Leaf $buildPath)
    $mappedQt = $qtDrive + '/' + (Split-Path -Leaf $qtPath)
    $env:PATH = "$env:SystemRoot/System32;$env:SystemRoot;$env:SystemRoot/System32/WindowsPowerShell/v1.0;$gitDirectory"
    $environment = & $env:ComSpec /c ('call "' + $vsPath + '" -no_logo -arch=x64 -host_arch=x64 && set')
    if ($LASTEXITCODE -ne 0) { throw 'MSVC environment initialization failed' }
    foreach ($line in $environment) {
        if ($line -match '^([^=]+)=(.*)$') { [Environment]::SetEnvironmentVariable($matches[1], $matches[2], 'Process') }
    }
    $env:CL = '/MP1'
    Push-Location $mappedBuild
    $pushed = $true
    # Qt 6.8.3's dependency scan overflowed on this large source tree. Disable
    # that optional scan and force ALL objects below: header changes must never
    # leave stale objects. This is a full component build, not incremental.
    $configure = Start-Process -FilePath ($mappedQt + '/bin/qmake.exe') -ArgumentList @('-nodepend', ('"' + $mappedSource + '/OpenRGB.pro"'), 'CONFIG+=release', 'CONFIG-=debug', 'CONFIG-=debug_and_release', 'CONFIG+=prismestra_pc_backend') -WindowStyle Hidden -PassThru -RedirectStandardOutput 'configure.stdout.log' -RedirectStandardError 'configure.log'
    $configure.WaitForExit()
    $configureExit = $configure.ExitCode
    if ($configureExit -ne 0) { throw "qmake failed ($configureExit); inspect configure.log" }
    $compile = Start-Process -FilePath $jomPath -ArgumentList @('/A', ('-j' + $Jobs)) -WindowStyle Hidden -PassThru -RedirectStandardOutput 'build.log' -RedirectStandardError 'build.stderr.log'
    $compile.WaitForExit()
    $buildExit = $compile.ExitCode
    if ($buildExit -ne 0) { throw "Compilation failed ($buildExit); inspect build.log" }
    Get-FileHash -LiteralPath 'release/OpenRGB.exe'
    # Runtime dependency staging, licenses and source verification are separate
    # gates. This script does not install, run, or approve the built component.
} finally {
    if ($pushed) { Pop-Location }
    foreach ($drive in $mappings) { & subst.exe $drive /D | Out-Null }
    if ($buildLock) { $buildLock.Dispose() }
}
