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
if ($LASTEXITCODE -ne 0 -or $masterCommit -cnotmatch '^[0-9a-f]{40}$') {
    throw 'Could not verify the live master ref.'
}
if ($masterCommit -cne $Commit) {
    $comparison = gh api --method GET "repos/$Repository/compare/$Commit...$masterCommit" -F per_page=1 `
        --jq "if .merge_base_commit.sha == `"$Commit`" then .status else `"wrong-base`" end"
    if ($LASTEXITCODE -ne 0 -or $comparison -cne 'ahead') {
        throw 'The release commit must be reachable from master.'
    }
}

$masterCheck = gh api --method GET "repos/$Repository/actions/workflows/windows.yml/runs" `
    -f branch=master -f event=push -f "head_sha=$Commit" -F per_page=1 `
    --jq '.workflow_runs[0].conclusion'
if ($LASTEXITCODE -ne 0 -or $masterCheck -cne 'success') {
    throw 'The exact release commit must have successful Windows CI on a master push.'
}

$publishedTags = gh api --paginate --method GET "repos/$Repository/releases" -F per_page=100 `
    --jq '.[] | select(.draft == false and .prerelease == false) | .tag_name'
if ($LASTEXITCODE -ne 0) { throw 'Could not read the public release versions.' }
$candidateVersion = [version]$Tag.Substring(1)
foreach ($publishedTag in $publishedTags) {
    if ($publishedTag -cmatch '^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$' -and
        $candidateVersion -le [version]$publishedTag.Substring(1)) {
        throw "The release version must be newer than the already published $publishedTag."
    }
}

[pscustomobject]@{ Version = $version; TagObject = $tagObject }
