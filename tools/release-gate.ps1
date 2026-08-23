param([switch]$Package)

$ErrorActionPreference = "Stop"
$runner = Join-Path $PSScriptRoot "test.ps1"
& $runner -Tier full
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
if ($Package) {
    & (Join-Path $PSScriptRoot "..\packaging\package.ps1")
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}
Write-Host "[release-gate] PASS"
