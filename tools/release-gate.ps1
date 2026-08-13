param(
    [switch]$Package
)

$ErrorActionPreference = "Stop"
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))

function Invoke-Checked {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][scriptblock]$Command
    )
    Write-Host "[release-gate] $Name"
    & $Command
    if ($LASTEXITCODE -ne 0) {
        throw "$Name failed with exit code $LASTEXITCODE"
    }
}

Push-Location $projectRoot
try {
    $env:CARGO_BUILD_JOBS = "1"
    # The Windows GNU debug graph (Iced + DX12 + media) can exhaust a small
    # system page file when incremental metadata from several target profiles
    # is mapped at once. Release validation favors deterministic low-memory
    # builds over incremental speed.
    $env:CARGO_INCREMENTAL = "0"
    Invoke-Checked "Formatting" { cargo fmt --all -- --check }
    Invoke-Checked "All-target check" { cargo check --all-targets --locked }
    Invoke-Checked "License metadata audit" { & (Join-Path $projectRoot "tools\audit-licenses.ps1") }
    Invoke-Checked "RustSec vulnerability audit" { & (Join-Path $projectRoot "tools\audit-rustsec.ps1") }
    Invoke-Checked "All-target tests" { cargo test --all-targets --locked }
    Invoke-Checked "Clippy" { cargo clippy --all-targets --all-features --locked -- -D warnings }

    if ($Package) {
        Invoke-Checked "Release packaging" {
            & (Join-Path $projectRoot "packaging\package.ps1")
        }
    }

    Write-Host "[release-gate] PASS"
} finally {
    Pop-Location
}
