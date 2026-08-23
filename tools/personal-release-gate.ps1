param([switch]$Package)

$ErrorActionPreference = "Stop"
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$metadata = cargo metadata --manifest-path (Join-Path $projectRoot "Cargo.toml") --no-deps --locked --format-version 1 | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) {
    throw "cargo metadata failed"
}
$packageMetadata = $metadata.packages | Where-Object name -eq "ghitabrowser" | Select-Object -First 1
if (-not $packageMetadata -or $packageMetadata.version -ne "2.0.6") {
    throw "Personal release gate requires package version 2.0.6"
}

$runner = Join-Path $PSScriptRoot "test.ps1"
& $runner -Tier release -Package:$Package
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}
Write-Host "[personal-release] PASS v$($packageMetadata.version)"
