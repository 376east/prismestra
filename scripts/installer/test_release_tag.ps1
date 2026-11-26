$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

$verifier = Join-Path $PSScriptRoot verify_release_tag.ps1
$fixture = Join-Path ([System.IO.Path]::GetTempPath()) "prismestra-release-tag-$([guid]::NewGuid())"
New-Item -ItemType Directory -Path $fixture | Out-Null
Push-Location -LiteralPath $fixture
try {
    git init --quiet
    git -c user.name=Fixture -c user.email=fixture@example.invalid -c commit.gpgsign=false commit --quiet --allow-empty -m Fixture
    $fixtureCommit = git rev-parse HEAD
    git -c user.name=Fixture -c user.email=fixture@example.invalid -c tag.gpgsign=false tag -a v1.2.3 -m Fixture
    git -c tag.gpgsign=false tag v1.2.4
    $fixtureState = @{
        Tag = git rev-parse refs/tags/v1.2.3
        Master = $fixtureCommit
        Conclusion = 'success'
    }
    $repository = 'fixture/prismestra'

    function gh {
        $global:LASTEXITCODE = 0
        $request = $args -join ' '
        switch -Exact ($request) {
            "api repos/$repository/git/ref/tags/v1.2.3 --jq .object.sha" { $fixtureState.Tag }
            "api repos/$repository/git/ref/heads/master --jq .object.sha" { $fixtureState.Master }
            "api --method GET repos/$repository/actions/workflows/windows.yml/runs -f branch=master -f event=push -f head_sha=$fixtureCommit -F per_page=1 --jq .workflow_runs[0].conclusion" {
                $fixtureState.Conclusion
            }
            default { throw "Unexpected GitHub request: $request" }
        }
    }

    function Assert-Rejected([string]$Tag, [string]$Message, [string]$ExpectedTagObject) {
        $params = @{ Tag = $Tag; Commit = $fixtureCommit; Repository = $repository }
        if ($ExpectedTagObject) { $params.ExpectedTagObject = $ExpectedTagObject }
        try {
            $null = & $verifier @params
        } catch {
            if ($_.Exception.Message.Contains($Message)) { return }
            throw
        }
        throw "Expected $Tag to be rejected: $Message"
    }

    $result = & $verifier -Tag v1.2.3 -Commit $fixtureCommit -Repository $repository
    if ($result.Version -cne '1.2.3.0' -or $result.TagObject -cne $fixtureState.Tag) {
        throw 'The verified release did not return the correct version and tag identity.'
    }
    $otherObject = '0' * 40
    Assert-Rejected 'v1.2.3-preview.1' 'stable vMAJOR.MINOR.PATCH'
    Assert-Rejected 'v01.2.3' 'stable vMAJOR.MINOR.PATCH'
    Assert-Rejected 'v65536.2.3' 'stable vMAJOR.MINOR.PATCH'
    Assert-Rejected 'v1.2.4' 'requires an annotated tag'
    Assert-Rejected 'v1.2.3' 'match the release inputs' $otherObject
    $fixtureState.Tag = $otherObject
    Assert-Rejected 'v1.2.3' 'live release tag differs'
    $fixtureState.Tag = $result.TagObject
    $fixtureState.Master = $otherObject
    Assert-Rejected 'v1.2.3' 'current master tip'
    $fixtureState.Master = $fixtureCommit
    $fixtureState.Conclusion = 'failure'
    Assert-Rejected 'v1.2.3' 'successful Windows CI'
    $fixtureState.Conclusion = $null
    Assert-Rejected 'v1.2.3' 'successful Windows CI'
    git -c user.name=Fixture -c user.email=fixture@example.invalid -c commit.gpgsign=false commit --quiet --allow-empty -m 'Different checkout'
    Assert-Rejected 'v1.2.3' 'match the release inputs'
    Write-Output 'Release tag verification passed.'
} finally {
    Pop-Location
    $resolved = (Resolve-Path -LiteralPath $fixture).Path
    if ($resolved -ceq $fixture) { Remove-Item -LiteralPath $resolved -Recurse -Force }
}
