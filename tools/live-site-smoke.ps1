param(
    [string]$OutputPath = ".\dist\live-site-report.json"
)

$ErrorActionPreference = "Stop"
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$browser = Join-Path $projectRoot "target\release\ghitabrowser.exe"
$matrix = Join-Path $projectRoot "tests\fixtures\web\live-site-matrix.json"
$output = [IO.Path]::GetFullPath((Join-Path $projectRoot $OutputPath))

if (-not (Test-Path -LiteralPath $browser -PathType Leaf)) {
    throw "Release browser not found: $browser"
}

New-Item -ItemType Directory -Force -Path (Split-Path -Parent $output) | Out-Null
if (Test-Path -LiteralPath $output -PathType Leaf) {
    Remove-Item -LiteralPath $output -Force
}
$probe = Start-Process -FilePath $browser -ArgumentList @(
    "--compatibility-probe=$matrix",
    "--compatibility-report=$output"
) -Wait -PassThru -WindowStyle Hidden
if ($probe.ExitCode -ne 0) {
    throw "Live compatibility probe failed with exit code $($probe.ExitCode)"
}
if (-not (Test-Path -LiteralPath $output -PathType Leaf)) {
    throw "Live compatibility probe did not create report: $output"
}

$report = Get-Content -LiteralPath $output -Raw | ConvertFrom-Json
if ($report.summary.total -ne 12) {
    throw "Live matrix must contain exactly 12 results"
}
if ($report.summary.timed_out -ne 0 -or $report.summary.usable_percent -lt 90) {
    throw "Live matrix failed: $($report.summary | ConvertTo-Json -Compress)"
}

Write-Host "[live-site] PASS $($report.summary.usable_percent)% usable/readable"
