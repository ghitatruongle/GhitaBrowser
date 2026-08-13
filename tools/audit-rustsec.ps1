$ErrorActionPreference = 'Stop'

$version = '0.22.2'
$expectedSha256 = '0a7316540862c13d954f648917ceacca593747baed6eec180fafa590be2710ab'
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$toolRoot = Join-Path $projectRoot "target\release-tools\cargo-audit-$version"
$archive = Join-Path $toolRoot 'cargo-audit.zip'
$expandedRoot = Join-Path $toolRoot "cargo-audit-x86_64-pc-windows-msvc-v$version"
$executable = Join-Path $expandedRoot 'cargo-audit.exe'
$download = "https://github.com/rustsec/rustsec/releases/download/cargo-audit/v$version/cargo-audit-x86_64-pc-windows-msvc-v$version.zip"

if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
    New-Item -ItemType Directory -Path $toolRoot -Force | Out-Null
    Invoke-WebRequest -Uri $download -OutFile $archive -Headers @{
        'User-Agent' = 'GhitaBrowser-release-audit'
    }
    $actualSha256 = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actualSha256 -ne $expectedSha256) {
        throw "cargo-audit archive checksum mismatch: expected $expectedSha256, got $actualSha256"
    }
    Expand-Archive -LiteralPath $archive -DestinationPath $toolRoot -Force
}

if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
    throw "Pinned cargo-audit executable is missing after extraction: $executable"
}

$raw = & $executable audit --json 2>&1
$auditExit = $LASTEXITCODE
try {
    $report = ($raw -join "`n") | ConvertFrom-Json
} catch {
    throw "cargo-audit returned invalid JSON: $($raw -join "`n")"
}

$vulnerabilityCount = @($report.vulnerabilities.list).Count
$warningCount = 0
foreach ($warningKind in $report.warnings.PSObject.Properties) {
    $warningCount += @($warningKind.Value).Count
}

if ($auditExit -ne 0 -or $report.vulnerabilities.found -or $vulnerabilityCount -ne 0) {
    throw "RustSec audit found $vulnerabilityCount locked vulnerability/vulnerabilities (exit $auditExit)"
}

$advisoryCount = $report.database.'advisory-count'
$lastCommit = $report.database.'last-commit'
$lastUpdated = $report.database.'last-updated'
"RustSec audit passed: 0 vulnerabilities, $warningCount informational warnings, $advisoryCount advisories, database $lastCommit updated $lastUpdated."
