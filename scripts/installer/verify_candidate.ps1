[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Directory,
    [Parameter(Mandatory)][ValidatePattern('^[0-9a-f]{40}$')][string]$Commit,
    [Parameter(Mandatory)][ValidatePattern('^[0-9a-f]{64}$')][string]$SumsSha256
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path -LiteralPath $Directory).Path
$sums = Join-Path $root SHA256SUMS.txt
function Sha256([string]$Path) {
    (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
}
if ((Sha256 $sums) -cne $SumsSha256) { throw 'Checksum list differs from the build output.' }

$manifestPath = Join-Path $root install-manifest.json
$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
$version = $manifest.installer_version
if ($manifest.product -cne 'Prismestra' -or $manifest.commit -cne $Commit -or
    $null -ne $manifest.data_namespace -or $version -cnotmatch '^\d{1,5}(\.\d{1,5}){3}$') {
    throw 'Expected the production installer manifest for this commit.'
}
$setupName = "Prismestra-Setup-$version-x64.exe"
$expected = @($setupName, "Prismestra-Sources-$version.zip", 'install-manifest.json')
$actual = @(Get-ChildItem -LiteralPath $root -Force | ForEach-Object Name)
if (Compare-Object -ReferenceObject ($expected + 'SHA256SUMS.txt') -DifferenceObject $actual -CaseSensitive) {
    throw 'Unexpected candidate files.'
}
$hashes = @{}
foreach ($line in Get-Content -LiteralPath $sums) {
    if ($line -cnotmatch '^([0-9a-f]{64})  ([a-zA-Z0-9._-]+)$') { throw 'Invalid checksum entry.' }
    $hash, $name = $Matches[1], $Matches[2]
    if ($name -cnotin $expected -or $hashes.ContainsKey($name)) { throw "Unexpected checksum entry: $name" }
    if ((Sha256 (Join-Path $root $name)) -cne $hash) { throw "File SHA-256 mismatch: $name" }
    $hashes[$name] = $hash
}
if ($hashes.Count -ne $expected.Count) { throw 'Candidate is missing file checksums.' }
[pscustomobject]@{
    Setup = Join-Path $root $setupName
    SetupSha256 = $hashes[$setupName]
    ManifestSha256 = $hashes['install-manifest.json']
}
