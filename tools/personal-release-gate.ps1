param(
    [switch]$Package,
    # Optional pin: fail the gate unless Cargo.toml declares this exact
    # version. Leave empty to accept whatever the workspace declares —
    # Cargo.toml is the single source of truth; packaging/package.ps1
    # already asserts every staged binary reports that same version.
    [string]$RequireVersion
)

$ErrorActionPreference = "Stop"
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$metadata = cargo metadata --manifest-path (Join-Path $projectRoot "Cargo.toml") --no-deps --locked --format-version 1 | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) {
    throw "cargo metadata failed"
}
$packageMetadata = $metadata.packages | Where-Object name -eq "ghitabrowser" | Select-Object -First 1
if (-not $packageMetadata) {
    throw "Unable to read ghitabrowser package metadata"
}
if ($RequireVersion -and $packageMetadata.version -ne $RequireVersion) {
    throw "Personal release gate pinned to $RequireVersion but Cargo.toml declares $($packageMetadata.version)"
}

$runner = Join-Path $PSScriptRoot "test.ps1"
& $runner -Tier release -Package:$Package
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}
Write-Host "[personal-release] PASS v$($packageMetadata.version)"
