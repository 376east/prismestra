$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

$verifier = Join-Path $PSScriptRoot verify_release_tag.ps1
$publisher = Join-Path $PSScriptRoot publish_release.ps1
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
        Comparison = 'ahead'
        Releases = @(
            @{ Tag = 'v1.2.2'; Draft = $false; Prerelease = $false }
            @{ Tag = 'build-inputs-v1'; Draft = $false; Prerelease = $false }
            @{ Tag = 'v9.0.0'; Draft = $true; Prerelease = $false }
            @{ Tag = 'v8.0.0'; Draft = $false; Prerelease = $true }
        )
        Calls = @()
    }
    $repository = 'fixture/prismestra'

    function gh {
        $global:LASTEXITCODE = 0
        $request = $args -join ' '
        if ($args[0] -ceq 'release' -and $args[2] -ceq 'v1.2.3') {
            $fixtureState.Calls += $args[1]
            switch -Exact ($args[1]) {
                'create' {
                    if ($args -cnotcontains '--verify-tag' -or $args -cnotcontains '--draft' -or $args -ccontains '--latest') {
                        throw 'Expected an unpublished draft for an existing tag.'
                    }
                    $fixtureState.Draft = [ordered]@{
                        id = 42; tag_name = 'v1.2.3'; draft = $true; prerelease = $false
                        name = $args[$args.IndexOf('--title') + 1]
                        body = $args[$args.IndexOf('--notes') + 1]
                        assets = @()
                    }
                }
                'upload' {
                    foreach ($path in $args[3..($args.IndexOf('--repo') - 1)]) {
                        $digest = (Get-FileHash -LiteralPath $path).Hash.ToLowerInvariant()
                        if ($fixtureState.Corrupt) { $digest = '0' * 64 }
                        $fixtureState.Draft.assets += @{ name = Split-Path -Leaf $path; state = 'uploaded'; digest = "sha256:$digest" }
                    }
                    if ($fixtureState.AdvanceVersion) {
                        $fixtureState.Releases += @{ Tag = 'v1.2.4'; Draft = $false; Prerelease = $false }
                    }
                }
                default { throw "Unexpected GitHub release request: $request" }
            }
            return
        }
        switch -Exact ($request) {
            "api repos/$repository/git/ref/tags/v1.2.3 --jq .object.sha" { $fixtureState.Tag }
            "api repos/$repository/git/ref/heads/master --jq .object.sha" { $fixtureState.Master }
            "api --method GET repos/$repository/compare/$fixtureCommit...$($fixtureState.Master) -F per_page=1 --jq if .merge_base_commit.sha == `"$fixtureCommit`" then .status else `"wrong-base`" end" {
                $fixtureState.Comparison
            }
            "api --method GET repos/$repository/actions/workflows/windows.yml/runs -f branch=master -f event=push -f head_sha=$fixtureCommit -F per_page=1 --jq .workflow_runs[0].conclusion" {
                $fixtureState.Conclusion
            }
            "api --paginate --method GET repos/$repository/releases -F per_page=100 --jq .[] | select(.draft == false and .prerelease == false) | .tag_name" {
                $fixtureState.Releases | Where-Object { !$_.Draft -and !$_.Prerelease } | ForEach-Object Tag
            }
            "api --paginate --method GET repos/$repository/releases -F per_page=100 --jq .[] | select(.tag_name == `"v1.2.3`") | .id" {
                if ($fixtureState.Draft) { $fixtureState.Draft.id }
            }
            "api repos/$repository/releases/42" { $fixtureState.Draft | ConvertTo-Json -Depth 4 -Compress }
            "api --method PATCH repos/$repository/releases/42 -F draft=false -F prerelease=false -f make_latest=true --jq .html_url" {
                $fixtureState.Calls += 'publish'
                $fixtureState.Draft.draft = $false
                'https://example.invalid/release/v1.2.3'
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
    $null = & $verifier -Tag v1.2.3 -Commit $fixtureCommit -Repository $repository
    $fixtureState.Comparison = 'diverged'
    Assert-Rejected 'v1.2.3' 'reachable from master'
    $fixtureState.Comparison = 'wrong-base'
    Assert-Rejected 'v1.2.3' 'reachable from master'
    $fixtureState.Comparison = 'ahead'
    $fixtureState.Conclusion = 'failure'
    Assert-Rejected 'v1.2.3' 'successful Windows CI'
    $fixtureState.Conclusion = $null
    Assert-Rejected 'v1.2.3' 'successful Windows CI'
    $fixtureState.Conclusion = 'success'
    $originalReleases = $fixtureState.Releases
    $fixtureState.Releases += @{ Tag = 'v1.2.10'; Draft = $false; Prerelease = $false }
    Assert-Rejected 'v1.2.3' 'newer than the already published v1.2.10'
    $fixtureState.Releases = $originalReleases + @{ Tag = 'v1.2.3'; Draft = $false; Prerelease = $false }
    Assert-Rejected 'v1.2.3' 'newer than the already published v1.2.3'
    $fixtureState.Releases = $originalReleases

    $payload = New-Item -ItemType Directory -Path (Join-Path $fixture candidate)
    foreach ($file in 'Prismestra-Setup-1.2.3.0-x64.exe', 'Prismestra-Setup-Offline-1.2.3.0-x64.exe',
            'Prismestra-Sources-1.2.3.0.zip') {
        Set-Content -LiteralPath (Join-Path $payload.FullName $file) -Value $file
    }
    $manifest = @{ product = 'Prismestra'; commit = $fixtureCommit; installer_version = '1.2.3.0'
        app_sha256 = 'a' * 64; backend = @{ sha256 = 'b' * 64 } }
    $manifest.prerequisites = @{ webview2_bootstrapper = @{ file = 'MicrosoftEdgeWebview2Setup.exe' } }
    $manifest | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $payload.FullName 'install-manifest.json')
    $manifest.prerequisites = @{ webview2_x64 = @{ file = 'MicrosoftEdgeWebView2RuntimeInstallerX64.exe' } }
    $manifest | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $payload.FullName 'offline-install-manifest.json')
    $sums = Join-Path $payload.FullName 'SHA256SUMS.txt'
    Get-ChildItem -LiteralPath $payload.FullName -File | ForEach-Object {
        "$((Get-FileHash -LiteralPath $_.FullName).Hash.ToLowerInvariant())  $($_.Name)"
    } | Set-Content -LiteralPath $sums
    $publishInputs = @{
        Directory = $payload.FullName; Tag = 'v1.2.3'; Commit = $fixtureCommit; Repository = $repository
        TagObject = $result.TagObject; SumsSha256 = (Get-FileHash -LiteralPath $sums).Hash.ToLowerInvariant(); RunId = '180'
    }
    function Assert-PublishRejected([string]$Message) {
        try {
            $null = & $publisher @publishInputs
        } catch {
            if ($_.Exception.Message.Contains($Message) -and $fixtureState.Calls -cnotcontains 'publish') { return }
            throw
        }
        throw "Expected publication to fail before promotion: $Message"
    }

    $fixtureState.Corrupt = $true
    Assert-PublishRejected 'Draft asset does not match the verified candidate'
    $fixtureState.Draft = $null
    $fixtureState.Calls = @()
    $fixtureState.Corrupt = $false
    $fixtureState.AdvanceVersion = $true
    Assert-PublishRejected 'newer than the already published v1.2.4'
    if (($fixtureState.Calls -join ',') -cne 'create,upload' -or $fixtureState.Draft.assets.Count -ne 3) {
        throw 'Expected verified draft assets to be retained after a newer publication.'
    }
    $fixtureState.Calls = @()
    $fixtureState.Releases = $originalReleases
    $null = & $publisher @publishInputs
    if (($fixtureState.Calls -join ',') -cne 'publish') { throw 'Expected verified draft to resume without uploading again.' }

    git -c user.name=Fixture -c user.email=fixture@example.invalid -c commit.gpgsign=false commit --quiet --allow-empty -m 'Different checkout'
    Assert-Rejected 'v1.2.3' 'match the release inputs'
    Write-Output 'Release tag verification passed.'
} finally {
    Pop-Location
    $resolved = (Resolve-Path -LiteralPath $fixture).Path
    if ($resolved -ceq $fixture) { Remove-Item -LiteralPath $resolved -Recurse -Force }
}
