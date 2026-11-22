$ErrorActionPreference = 'Stop'

if (!$env:ImageOS -or !$env:ImageVersion -or $env:RUNNER_ARCH -ne 'X64') { throw 'Expected an exact hosted Windows x64 image' }
$selected = @{}
& $env:ComSpec /c ('call "' + $env:VSDEVCMD + '" -no_logo -arch=x64 -host_arch=x64 >NUL && set') | ForEach-Object {
  if ($_ -match '^(VCToolsInstallDir|VCToolsVersion|WindowsSdkDir|WindowsSDKVersion)=(.+)$') {
    $selected[$Matches[1]] = $Matches[2]
  }
}
if ($LASTEXITCODE -ne 0 -or $selected.Count -ne 4) { throw 'MSVC or Windows SDK version could not be established' }
$compiler = Join-Path $selected.VCToolsInstallDir 'bin/Hostx64/x64'
$sdk = Join-Path (Join-Path $selected.WindowsSdkDir 'bin') ($selected.WindowsSDKVersion.TrimEnd('\') + '/x64')
$key = python scripts/openrgb_cache.py --image "$env:ImageOS/${env:ImageVersion}" `
  --toolchain "$($selected.VCToolsVersion)/$($selected.WindowsSDKVersion)" `
  --tool vsdevcmd $env:VSDEVCMD --tool cl (Join-Path $compiler 'cl.exe') `
  --tool link (Join-Path $compiler 'link.exe') --tool lib (Join-Path $compiler 'lib.exe') `
  --tool rc (Join-Path $sdk 'rc.exe') --tool mt (Join-Path $sdk 'mt.exe') `
  --tool 7z (Join-Path $env:ProgramFiles '7-Zip/7z.exe') `
  --tool git (Get-Command git -CommandType Application | Select-Object -First 1).Source `
  --tool powershell (Join-Path $env:SystemRoot 'System32/WindowsPowerShell/v1.0/powershell.exe')
if ($LASTEXITCODE -ne 0 -or $key -cnotmatch '^openrgb-windows-x64-v2-[0-9a-f]{64}$') {
  throw 'OpenRGB cache key could not be established'
}
"key=$key" >> $env:GITHUB_OUTPUT
