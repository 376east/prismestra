[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Directory,
    [Parameter(Mandatory)][ValidatePattern('^[0-9a-f]{40}$')][string]$Commit,
    [Parameter(Mandatory)][ValidatePattern('^[0-9a-f]{64}$')][string]$SetupSha256,
    [Parameter(Mandatory)][ValidatePattern('^[0-9a-f]{64}$')][string]$ManifestSha256,
    [Parameter(Mandatory)][ValidatePattern('^[0-9a-f]{64}$')][string]$ProbeSha256
)

$ErrorActionPreference = 'Stop'
function Read-Receipt([string]$Name, [string]$Scope) {
    $path = Join-Path $Directory $Name
    if (!(Test-Path -LiteralPath $path -PathType Leaf)) { throw "Missing upgrade receipt: $Name" }
    $receipt = Get-Content -LiteralPath $path -Raw | ConvertFrom-Json
    if ($receipt.commit -cne $Commit -or $receipt.installer_sha256 -cne $SetupSha256 -or
        $receipt.result -cne 'passed' -or $receipt.scope -cne $Scope) {
        throw "Upgrade receipt is incomplete or does not match the candidate: $Name"
    }
    return $receipt
}

$upgrade = Read-Receipt 'production-upgrade.json' 'production_upgrade_lifecycle'
if ($upgrade.manifest_sha256 -cne $ManifestSha256 -or $upgrade.upgrade_verified -isnot [bool] -or
    !$upgrade.upgrade_verified -or $upgrade.uninstall_verified -isnot [bool] -or !$upgrade.uninstall_verified) {
    throw 'The packaged upgrade or uninstall was not verified against this manifest.'
}
foreach ($file in 'profile.json', 'state/sensor-sources.json', 'state/lighting-local.json', 'state/assets/ci-upgrade.txt') {
    if ($upgrade.saved_data_sha256.$file -cnotmatch '^[0-9a-f]{64}$') {
        throw "Missing upgrade evidence for synthetic profile data: $file"
    }
}

$routes = Read-Receipt 'temperature-routes.json' 'saved_catalog_resolution'
if ($routes.probe_sha256 -cne $ProbeSha256 -or
    $routes.before.scope_sha256 -cnotmatch '^[0-9a-f]{64}$' -or
    $routes.before.scope_sha256 -cne $routes.after.scope_sha256 -or
    $routes.before.routes.Count -ne 5 -or $routes.after.routes.Count -ne 5) {
    throw 'Missing the five temperature assignments and matching profile scope.'
}
for ($i = 0; $i -lt 5; $i++) {
    $before = $routes.before.routes[$i]
    $after = $routes.after.routes[$i]
    if ($before.input -cne "ci_sensor_$i" -or $after.input -cne $before.input -or
        $before.source -cne "physical/ci-sensor-$i" -or $after.source -cne $before.source -or
        $before.revision -ne 1 -or $after.revision -ne $before.revision) {
        throw "Temperature assignment $i is missing or changed."
    }
}

$configuration = Read-Receipt 'openrgb-configuration.json' 'preexisting_protected_fixture'
if ($configuration.before.sha256 -cnotmatch '^[0-9a-f]{64}$' -or
    $configuration.before.acl_sha256 -cnotmatch '^[0-9a-f]{64}$' -or
    $configuration.before.sha256 -cne $configuration.after.sha256 -or
    $configuration.before.acl_sha256 -cne $configuration.after.acl_sha256) {
    throw 'Missing matching bytes and ACL evidence for protected OpenRGB Configuration.json.'
}
