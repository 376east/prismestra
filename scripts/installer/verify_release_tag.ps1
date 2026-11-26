[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Tag,
    [Parameter(Mandatory)][ValidatePattern('^[0-9a-f]{40}$')][string]$Commit,
    [Parameter(Mandatory)][string]$Repository,
    [ValidatePattern('^[0-9a-f]{40}$')][string]$ExpectedTagObject
)

$ErrorActionPreference = 'Stop'
if ($Tag -cnotmatch '^v(0|[1-9][0-9]{0,4})\.(0|[1-9][0-9]{0,4})\.(0|[1-9][0-9]{0,4})$' -or
    @($Matches[1], $Matches[2], $Matches[3]).Where({ [int]$_ -gt 65535 }).Count) {
    throw 'Expected a stable vMAJOR.MINOR.PATCH tag that fits the Windows version fields.'
}
$version = "$($Tag.Substring(1)).0"
$ref = "refs/tags/$Tag"
$type = git cat-file -t $ref
if ($LASTEXITCODE -ne 0 -or $type -cne 'tag') { throw 'The release requires an annotated tag.' }
$tagObject, $tagCommit, $head = git rev-parse $ref "${ref}^{commit}" HEAD
if ($LASTEXITCODE -ne 0 -or $tagCommit -cne $Commit -or $head -cne $Commit -or
    ($ExpectedTagObject -and $tagObject -cne $ExpectedTagObject)) {
    throw 'The checked-out commit and tag must match the release inputs.'
}
$remoteTagObject = gh api "repos/$Repository/git/ref/tags/$Tag" --jq .object.sha
if ($LASTEXITCODE -ne 0 -or $remoteTagObject -cne $tagObject) {
    throw 'The live release tag differs from the checked-out tag.'
}
$masterCommit = gh api "repos/$Repository/git/ref/heads/master" --jq .object.sha
if ($LASTEXITCODE -ne 0 -or $masterCommit -cne $Commit) {
    throw 'The release commit must be the current master tip.'
}

$masterCheck = gh api --method GET "repos/$Repository/actions/workflows/windows.yml/runs" `
    -f branch=master -f event=push -f "head_sha=$Commit" -F per_page=1 `
    --jq '.workflow_runs[0].conclusion'
if ($LASTEXITCODE -ne 0 -or $masterCheck -cne 'success') {
    throw 'The exact release commit must have successful Windows CI on a master push.'
}

[pscustomobject]@{ Version = $version; TagObject = $tagObject }
