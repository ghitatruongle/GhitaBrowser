param(
    [string]$ExecutablePath = ".\target\release\ghitabrowser.exe",
    [string]$ReportPath
)

$ErrorActionPreference = "Stop"
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$metadata = cargo metadata --manifest-path (Join-Path $projectRoot "Cargo.toml") --no-deps --locked --format-version 1 | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw "cargo metadata failed" }
$packageMetadata = $metadata.packages | Where-Object name -eq "ghitabrowser" | Select-Object -First 1
if (-not $packageMetadata) { throw "ghitabrowser package metadata is missing" }
$version = [string]$packageMetadata.version
if (-not $ReportPath) {
    $ReportPath = ".\dist\personal-release-smoke-$version.json"
}
$executable = [IO.Path]::GetFullPath((Join-Path $projectRoot $ExecutablePath))
$report = [IO.Path]::GetFullPath((Join-Path $projectRoot $ReportPath))
if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
    throw "Release executable not found: $executable"
}
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $report) | Out-Null
$smoke = Start-Process -FilePath $executable -ArgumentList "--release-smoke-report=$report" -Wait -PassThru -WindowStyle Hidden
if ($smoke.ExitCode -ne 0) {
    throw "Release smoke exited with $($smoke.ExitCode)"
}
if (-not (Test-Path -LiteralPath $report -PathType Leaf)) {
    throw "Release smoke did not create report: $report"
}
$data = Get-Content -LiteralPath $report -Raw | ConvertFrom-Json
if (-not $data.passed -or -not $data.worker -or -not $data.runtime -or -not $data.scene) {
    throw "Release smoke report did not pass every required subsystem"
}
if (-not ([string]$data.version).StartsWith($version, [StringComparison]::Ordinal)) {
    throw "Release smoke version '$($data.version)' is not $version"
}
Write-Host "[release-smoke] PASS v$($data.version)"
