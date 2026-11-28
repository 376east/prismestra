[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Directory,
    [Parameter(Mandatory)][string]$Tag,
    [Parameter(Mandatory)][string]$Commit,
    [Parameter(Mandatory)][string]$Repository,
    [Parameter(Mandatory)][string]$TagObject,
    [Parameter(Mandatory)][string]$SumsSha256,
    [Parameter(Mandatory)][ValidatePattern('^[0-9]+$')][string]$RunId
)

$ErrorActionPreference = 'Stop'
$verifier = Join-Path $PSScriptRoot verify_release_tag.ps1
$tagInputs = @{ Tag = $Tag; Commit = $Commit; Repository = $Repository; ExpectedTagObject = $TagObject }
$release = & $verifier @tagInputs
$candidate = & (Join-Path $PSScriptRoot verify_candidate.ps1) -Directory $Directory -Commit $Commit -SumsSha256 $SumsSha256
$setupName = "Prismestra-Setup-$($release.Version)-x64.exe"
if ((Split-Path -Leaf $candidate.Setup) -cne $setupName) {
    throw 'The tested installer version differs from the release tag.'
}
$sourceName = "Prismestra-Sources-$($release.Version).zip"
$sources = Join-Path $Directory $sourceName
$offlineName = Split-Path -Leaf $candidate.OfflineSetup
$expected = @{}
$expected[$setupName] = @{ Path = $candidate.Setup; Sha256 = $candidate.SetupSha256 }
$expected[$offlineName] = @{ Path = $candidate.OfflineSetup; Sha256 = $candidate.OfflineSetupSha256 }
$expected[$sourceName] = @{ Path = $sources; Sha256 = (Get-FileHash -LiteralPath $sources).Hash.ToLowerInvariant() }

$title = "Prismestra $($Tag.Substring(1))"
$setupUrl = "https://github.com/$Repository/releases/download/$Tag/$setupName"
$offlineUrl = "https://github.com/$Repository/releases/download/$Tag/$offlineName"
$runUrl = "https://github.com/$Repository/actions/runs/$RunId"
$sourceUrl = "https://github.com/$Repository/blob/$Commit"
$marker = "<!-- Prismestra release run $RunId, commit $Commit, tag $TagObject -->"
$signature = Get-AuthenticodeSignature -LiteralPath $candidate.Setup
$signatureNote = if ($signature.Status -eq 'NotSigned') { 'The Setup is unsigned.' }
$notes = @"
**[Download Windows x64 Setup]($setupUrl)** · Windows 10 version 1903 or later, or Windows 11 (x64).

Setup includes OpenRGB and downloads Microsoft WebView2 only if needed. [Offline Setup]($offlineUrl) includes the full runtime. The ZIP contains the corresponding sources. $signatureNote

[Changelog]($sourceUrl/CHANGELOG.md) · [Device compatibility]($sourceUrl/docs/compatibility.md) · [Windows release checks]($runUrl)

Setup SHA-256: $($candidate.SetupSha256)
Offline Setup SHA-256: $($candidate.OfflineSetupSha256)
$marker
"@
$api = "repos/$Repository/releases"

function Find-ReleaseId {
    $ids = @(gh api --paginate --method GET $api -F per_page=100 `
        --jq ".[] | select(.tag_name == `"$Tag`") | .id")
    if ($LASTEXITCODE -ne 0 -or $ids.Count -gt 1 -or ($ids.Count -eq 1 -and $ids[0] -cnotmatch '^[0-9]+$')) {
        throw 'Could not identify a unique release for this tag.'
    }
    $ids
}

function Get-MissingAssets([string]$ReleaseId) {
    $data = gh api "$api/$ReleaseId"
    if ($LASTEXITCODE -ne 0) { throw 'Could not load the release draft.' }
    $draft = $data | ConvertFrom-Json
    if ([string]$draft.id -cne $ReleaseId -or $draft.tag_name -cne $Tag -or
        $draft.draft -ne $true -or $draft.prerelease -ne $false -or
        $draft.name -cne $title -or !([string]$draft.body).Contains($marker)) {
        throw 'The release draft does not belong to this verified tag and workflow run.'
    }
    $remaining = $expected.Clone()
    foreach ($asset in $draft.assets) {
        if ($asset.name -cnotin $remaining.Keys -or $asset.state -cne 'uploaded' -or
            $asset.digest -cne "sha256:$($remaining[$asset.name].Sha256)") {
            throw "Draft asset does not match the verified candidate: $($asset.name)"
        }
        $remaining.Remove($asset.name)
    }
    $remaining.Values | ForEach-Object { $_.Path }
}

$id = Find-ReleaseId
if (!$id) {
    $null = gh release create $Tag --repo $Repository --verify-tag --draft --title $title --notes $notes
    if ($LASTEXITCODE -ne 0) { throw 'Could not create the release draft.' }
    $id = Find-ReleaseId
    if (!$id) { throw 'The new release draft was not found.' }
}

$missing = @(Get-MissingAssets $id)
if ($missing.Count) {
    gh release upload $Tag @missing --repo $Repository
    if ($LASTEXITCODE -ne 0) { throw 'Could not upload the verified release assets.' }
    if (@(Get-MissingAssets $id).Count) { throw 'The release draft is missing verified assets.' }
}

$null = & $verifier @tagInputs
$url = gh api --method PATCH "$api/$id" -F draft=false -F prerelease=false -f make_latest=true --jq .html_url
if ($LASTEXITCODE -ne 0 -or !$url) { throw 'Could not publish the verified release draft.' }
$url
