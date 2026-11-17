<#
.SYNOPSIS
Builds only OpenRGB's pinned usermode HIDAPI and PawnIOLib dependencies.
.DESCRIPTION
Extract the source archives identified by components/openrgb/dependency-lock.json.
Verify their SHA256 before extraction; pass their extracted roots below. Requires
Windows, Visual Studio C++ x64 tools/Windows SDK, CMake and Ninja (no WDK).
Example (paths are supplied by the caller):
  ./scripts/components/build_openrgb_dependencies.ps1 -HidapiSource $hidapiSource `
    -PawnioSource $pawnioSource -OutputRoot $outputRoot -Cmake $cmake `
    -Ninja $ninja -VsDevCmd $vsDevCmd
Outputs artifacts/{hidapi,pawniolib}/{bin,lib,include} and build.log beneath
OutputRoot. Does not download, install, run, sign, or replace any dependency.
The lock pins reviewed distribution artifacts; a rebuild is not promised to be
byte-identical across toolchains or paths and requires fresh verification.
#>
param(
    [Parameter(Mandatory=$true)][string]$HidapiSource,
    [Parameter(Mandatory=$true)][string]$PawnioSource,
    [Parameter(Mandatory=$true)][string]$OutputRoot,
    [Parameter(Mandatory=$true)][string]$Cmake,
    [Parameter(Mandatory=$true)][string]$Ninja,
    [Parameter(Mandatory=$true)][string]$VsDevCmd
)
$ErrorActionPreference = 'Stop'
$hidapiPath = (Resolve-Path -LiteralPath $HidapiSource).ProviderPath
$pawnioPath = (Resolve-Path -LiteralPath $PawnioSource).ProviderPath
$cmakePath = (Resolve-Path -LiteralPath $Cmake).ProviderPath
$ninjaPath = (Resolve-Path -LiteralPath $Ninja).ProviderPath
$vsPath = (Resolve-Path -LiteralPath $VsDevCmd).ProviderPath
foreach ($file in @((Join-Path $hidapiPath 'windows/hid.c'), (Join-Path $pawnioPath 'PawnIOLib/PawnIOLib.cpp'))) {
    if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { throw "Missing source: $file" }
}
$outputPath = [IO.Path]::GetFullPath($OutputRoot)
foreach ($source in @($hidapiPath, $pawnioPath)) {
    if ($outputPath -eq $source -or $outputPath.StartsWith($source.TrimEnd('\','/') + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
        throw 'OutputRoot must be outside both source trees'
    }
}
New-Item -ItemType Directory -Path $outputPath -Force | Out-Null
$logPath = Join-Path $outputPath 'build.log'
$environmentBefore = @{}
Get-ChildItem Env: | ForEach-Object { $environmentBefore[$_.Name] = $_.Value }
$buildLock = $null
function Invoke-Cmake([string[]]$Arguments) {
    & $cmakePath @Arguments 2>&1 | Tee-Object -FilePath $logPath -Append
    if ($LASTEXITCODE -ne 0) { throw "CMake failed ($LASTEXITCODE); inspect $logPath" }
}
try {
    $buildLock = [IO.File]::Open((Join-Path $outputPath '.dependency-build.lock'), 'OpenOrCreate', 'ReadWrite', 'None')
    # Avoid cmd.exe's command-line limit when inheriting a large developer PATH.
    $env:PATH = "$env:SystemRoot/System32;$env:SystemRoot"
    $environment = & $env:ComSpec /d /c ('call "' + $vsPath + '" -no_logo -arch=x64 -host_arch=x64 && set')
    if ($LASTEXITCODE -ne 0) { throw 'MSVC environment initialization failed' }
    foreach ($line in $environment) {
        if ($line -match '^([^=]+)=(.*)$') { [Environment]::SetEnvironmentVariable($matches[1], $matches[2], 'Process') }
    }
    # Ambient compiler flags must not silently change the reviewed recipe.
    foreach ($name in @('CL','_CL_','LINK','_LINK_','CFLAGS','CXXFLAGS','LDFLAGS')) {
        [Environment]::SetEnvironmentVariable($name, $null, 'Process')
    }
    $hidapiBuild = Join-Path $outputPath 'build/hidapi'
    $pawnioBuild = Join-Path $outputPath 'build/pawniolib'
    Invoke-Cmake @('-S',$hidapiPath,'-B',$hidapiBuild,'-G','Ninja',"-DCMAKE_MAKE_PROGRAM=$ninjaPath",'-DCMAKE_BUILD_TYPE=Release','-DCMAKE_POLICY_VERSION_MINIMUM=3.5','-DCMAKE_POLICY_DEFAULT_CMP0091=NEW','-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded','-DHIDAPI_WITH_TESTS=OFF','-DHIDAPI_BUILD_PP_DATA_DUMP=OFF','-DHIDAPI_BUILD_HIDTEST=OFF')
    Invoke-Cmake @('--build',$hidapiBuild,'--verbose')
    Invoke-Cmake @('-S',(Join-Path $PSScriptRoot 'pawniolib'),'-B',$pawnioBuild,'-G','Ninja',"-DCMAKE_MAKE_PROGRAM=$ninjaPath",'-DCMAKE_BUILD_TYPE=Release',"-DPAWN_SOURCE:PATH=$pawnioPath")
    Invoke-Cmake @('--build',$pawnioBuild,'--verbose')
    foreach ($kind in @('hidapi','pawniolib')) {
        foreach ($folder in @('bin','lib','include')) {
            New-Item -ItemType Directory -Path (Join-Path $outputPath "artifacts/$kind/$folder") -Force | Out-Null
        }
    }
    foreach ($extension in @('dll','lib')) {
        $folder = if ($extension -eq 'dll') { 'bin' } else { 'lib' }
        Copy-Item -LiteralPath (Join-Path $hidapiBuild "src/windows/hidapi-hotplug.$extension") -Destination (Join-Path $outputPath "artifacts/hidapi/$folder/")
        Copy-Item -LiteralPath (Join-Path $pawnioBuild "PawnIOLib.$extension") -Destination (Join-Path $outputPath "artifacts/pawniolib/$folder/")
    }
    Copy-Item -LiteralPath (Join-Path $hidapiPath 'hidapi-hotplug/hidapi.h'),(Join-Path $hidapiPath 'windows/hidapi_winapi.h') -Destination (Join-Path $outputPath 'artifacts/hidapi/include/')
    Copy-Item -LiteralPath (Join-Path $pawnioPath 'PawnIOLib/include/PawnIOLib.h') -Destination (Join-Path $outputPath 'artifacts/pawniolib/include/')
    Get-FileHash -LiteralPath (Join-Path $outputPath 'artifacts/hidapi/bin/hidapi-hotplug.dll'),(Join-Path $outputPath 'artifacts/pawniolib/bin/PawnIOLib.dll')
} finally {
    if ($buildLock) { $buildLock.Dispose() }
    foreach ($entry in @(Get-ChildItem Env:)) {
        if (-not $environmentBefore.ContainsKey($entry.Name)) { [Environment]::SetEnvironmentVariable($entry.Name, $null, 'Process') }
    }
    foreach ($name in $environmentBefore.Keys) { [Environment]::SetEnvironmentVariable($name, $environmentBefore[$name], 'Process') }
}
