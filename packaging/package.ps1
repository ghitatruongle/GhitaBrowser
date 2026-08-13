# Build reproducible GhitaBrowser Windows release artifacts.
$ErrorActionPreference = "Stop"

$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$dist = [IO.Path]::GetFullPath((Join-Path $projectRoot "dist"))
if (-not $dist.StartsWith($projectRoot, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to package outside the project directory: $dist"
}

Push-Location $projectRoot
try {
    $metadata = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
    $package = $metadata.packages | Where-Object { $_.name -eq "ghitabrowser" } | Select-Object -First 1
    if (-not $package) {
        throw "Unable to read package metadata."
    }
    $version = $package.version

    cargo build --release --locked
    if ($LASTEXITCODE -ne 0) {
        throw "Release build failed."
    }

    New-Item -ItemType Directory -Force -Path $dist | Out-Null
    $stage = Join-Path $dist "GhitaBrowser-v$version-windows-x64"
    if (Test-Path -LiteralPath $stage) {
        Remove-Item -LiteralPath $stage -Recurse -Force
    }
    New-Item -ItemType Directory -Path $stage | Out-Null

    Copy-Item -LiteralPath (Join-Path $projectRoot "target\release\ghitabrowser.exe") -Destination (Join-Path $stage "GhitaBrowser.exe")
    Copy-Item -LiteralPath (Join-Path $projectRoot "target\release\ghita-renderer-worker.exe") -Destination $stage
    Copy-Item -LiteralPath (Join-Path $projectRoot "target\release\ghita-browser-child.exe") -Destination $stage
    Copy-Item -LiteralPath (Join-Path $projectRoot "icon.ico") -Destination $stage
    Copy-Item -LiteralPath (Join-Path $projectRoot "LICENSE") -Destination $stage
    Copy-Item -LiteralPath (Join-Path $projectRoot "THIRD_PARTY_NOTICES.md") -Destination $stage
    Copy-Item -LiteralPath (Join-Path $projectRoot "README.md") -Destination $stage
    Copy-Item -LiteralPath (Join-Path $projectRoot "packaging\Run-Portable.bat") -Destination $stage

    $archive = Join-Path $dist "GhitaBrowser-v$version-windows-x64.zip"
    if (Test-Path -LiteralPath $archive) {
        Remove-Item -LiteralPath $archive -Force
    }
    Compress-Archive -Path (Join-Path $stage "*") -DestinationPath $archive -CompressionLevel Optimal

    $hash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
    Set-Content -LiteralPath "$archive.sha256" -Value "$hash  $([IO.Path]::GetFileName($archive))" -Encoding ascii

    $innoCandidates = @(@(
        (Join-Path ${env:ProgramFiles(x86)} "Inno Setup 6\ISCC.exe"),
        (Join-Path $env:ProgramFiles "Inno Setup 6\ISCC.exe")
    ) | Where-Object { $_ -and (Test-Path -LiteralPath $_ -PathType Leaf) })
    if ($innoCandidates.Count -gt 0) {
        $innoCompiler = $innoCandidates | Select-Object -First 1
        & $innoCompiler "/DMyAppVersion=$version" (Join-Path $projectRoot "packaging\GhitaBrowser-Setup.iss")
        if ($LASTEXITCODE -ne 0) {
            throw "Inno Setup compilation failed."
        }
        $setup = Join-Path $dist "GhitaBrowser-v$version-Setup.exe"
        $setupHash = (Get-FileHash -LiteralPath $setup -Algorithm SHA256).Hash.ToLowerInvariant()
        Set-Content -LiteralPath "$setup.sha256" -Value "$setupHash  $([IO.Path]::GetFileName($setup))" -Encoding ascii
    } else {
        Write-Warning "Inno Setup 6 was not found; portable ZIP was created without a setup executable."
    }

    Write-Host "Created release artifacts in $dist"
} finally {
    Pop-Location
}
