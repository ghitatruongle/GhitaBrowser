param(
    [ValidateSet("fast", "release", "full")]
    [string]$Tier = "fast",
    [switch]$Package
)

$ErrorActionPreference = "Stop"
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$startedAt = [DateTime]::UtcNow
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$passed = $false
$commandMetrics = [Collections.Generic.List[object]]::new()

if ($Package -and $Tier -ne "release") {
    throw "-Package is only valid with -Tier release"
}

function Get-DirectoryBytes {
    param([Parameter(Mandatory = $true)][string]$Path)
    if (-not (Test-Path -LiteralPath $Path -PathType Container)) { return [uint64]0 }
    $sum = (Get-ChildItem -LiteralPath $Path -Recurse -File -ErrorAction SilentlyContinue |
        Measure-Object -Property Length -Sum).Sum
    if ($null -eq $sum) { return [uint64]0 }
    return [uint64]$sum
}

function Invoke-Checked {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][scriptblock]$Command
    )
    Write-Host "[test:$Tier] $Name"
    $timer = [Diagnostics.Stopwatch]::StartNew()
    try {
        $global:LASTEXITCODE = 0
        & $Command
        if ($LASTEXITCODE -ne 0) {
            throw "$Name failed with exit code $LASTEXITCODE"
        }
        $commandMetrics.Add([ordered]@{
            name = $Name
            passed = $true
            elapsed_seconds = [math]::Round($timer.Elapsed.TotalSeconds, 3)
        })
    } catch {
        $commandMetrics.Add([ordered]@{
            name = $Name
            passed = $false
            elapsed_seconds = [math]::Round($timer.Elapsed.TotalSeconds, 3)
        })
        throw
    } finally {
        $timer.Stop()
    }
}

Push-Location $projectRoot
try {
    $metadata = cargo metadata --format-version 1 --no-deps --locked | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw "cargo metadata failed" }
    $targetDirectory = if ($env:CARGO_TARGET_DIR) {
        [IO.Path]::GetFullPath($env:CARGO_TARGET_DIR)
    } else {
        [IO.Path]::GetFullPath($metadata.target_directory)
    }
    $targetRoot = [IO.Path]::GetPathRoot($targetDirectory)
    $driveName = $targetRoot.TrimEnd('\').TrimEnd(':')
    $freeBytesBefore = [uint64](Get-PSDrive -Name $driveName).Free
    if ($freeBytesBefore -lt 6GB) {
        throw "Need at least 6 GB free before compile; target is $targetDirectory and only $([math]::Round($freeBytesBefore / 1GB, 2)) GB is free"
    }

    $env:CARGO_BUILD_JOBS = "1"
    $env:CARGO_INCREMENTAL = "0"
    $env:RUST_MIN_STACK = "16777216"

    if ($Tier -in @("fast", "release")) {
        Invoke-Checked "Formatting" { cargo fmt --all -- --check }
        Invoke-Checked "Library tests" { cargo test --lib --locked -j 1 }
    }

    if ($Tier -eq "release") {
        Invoke-Checked "Build profile policy" { cargo test --test build_profile_policy_test --locked -j 1 }
        Invoke-Checked "Build metrics schema" { cargo test --test build_metrics_schema_test --locked -j 1 }
        Invoke-Checked "Build runner policy" { cargo test --test build_runner_policy_test --locked -j 1 }
        Invoke-Checked "Web compatibility" { cargo test --test web_compatibility_release_test --locked -j 1 }
        Invoke-Checked "Real-site acceptance" { cargo test --test real_site_acceptance_test --locked -j 1 }
        Invoke-Checked "Safe ad blocking" { cargo test --test adblock_page_integrity_test --locked -j 1 }
        Invoke-Checked "Adblock boundedness" { cargo test --test adblock_boundedness_test --locked -j 1 }
        Invoke-Checked "Tracking protection" { cargo test --test phase25_tracking_protection_test --locked -j 1 }
        Invoke-Checked "RAM pressure UI" { cargo test --test ram_pressure_ui_test --locked -j 1 }
        Invoke-Checked "RAM soak" { cargo test --test ram_budget_soak_test --locked -j 1 }
        Invoke-Checked "Media and navigation soak" { cargo test --test soak_test --locked -j 1 }
        Invoke-Checked "Release acceptance" { cargo test --test release_acceptance_test --locked -j 1 }
        Invoke-Checked "Crash recovery" { cargo test --test crash_recovery_test --locked -j 1 }
        Invoke-Checked "Network integration" { cargo test --test network_test --locked -j 1 }
        Invoke-Checked "Web support contract" { cargo test --test web_platform_conformance_test --locked -j 1 }
        Invoke-Checked "Dynamic rendering budget" { cargo test --test dynamic_rendering_acceptance_test --locked -j 1 phase14_mutation_frames_stay_inside_retained_memory_and_time_budgets }
        Invoke-Checked "Release build" { cargo build --release --locked -j 1 }
        Invoke-Checked "Release smoke" { & (Join-Path $projectRoot "tools\release-smoke.ps1") }
        if ($Package) {
            Invoke-Checked "Release packaging" { & (Join-Path $projectRoot "packaging\package.ps1") -SkipBuild }
        }
    }

    if ($Tier -eq "full") {
        Invoke-Checked "Formatting" { cargo fmt --all -- --check }
        Invoke-Checked "All-target tests" { cargo test --all-targets --locked -j 1 }
        Invoke-Checked "Clippy" { cargo clippy --all-targets --all-features --locked -j 1 -- -D warnings }
        Invoke-Checked "License metadata audit" { & (Join-Path $projectRoot "tools\audit-licenses.ps1") }
        Invoke-Checked "RustSec vulnerability audit" { & (Join-Path $projectRoot "tools\audit-rustsec.ps1") }
    }

    $passed = $true
    Write-Host "[test:$Tier] PASS"
} finally {
    $stopwatch.Stop()
    if ($targetDirectory) {
        try {
            $freeBytesAfter = [uint64](Get-PSDrive -Name $driveName).Free
            $metrics = [ordered]@{
                schema_version = 1
                tier = $Tier
                passed = $passed
                started_at_utc = $startedAt.ToString("o")
                elapsed_seconds = [math]::Round($stopwatch.Elapsed.TotalSeconds, 3)
                target_directory = $targetDirectory
                target_bytes = Get-DirectoryBytes -Path $targetDirectory
                debug_bytes = Get-DirectoryBytes -Path (Join-Path $targetDirectory "debug")
                free_bytes_before = $freeBytesBefore
                free_bytes_after = $freeBytesAfter
                commands = @($commandMetrics)
            }
            $metricsDirectory = Join-Path $projectRoot "dist\build-metrics"
            New-Item -ItemType Directory -Force -Path $metricsDirectory | Out-Null
            $stamp = $startedAt.ToString("yyyyMMddTHHmmssfffZ")
            $metricsPath = Join-Path $metricsDirectory "$stamp-$Tier.json"
            $metrics | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $metricsPath -Encoding utf8
            Write-Host "[test:$Tier] Metrics: $metricsPath"
        } catch {
            Write-Warning "Unable to write build metrics: $($_.Exception.Message)"
        }
    }
    Pop-Location
}
