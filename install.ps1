# Local GhitaBrowser developer installer.
$ErrorActionPreference = "Stop"

$projectRoot = $PSScriptRoot
$metadata = cargo metadata --no-deps --locked --format-version 1 --manifest-path (Join-Path $projectRoot "Cargo.toml") | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw "cargo metadata failed" }
$package = $metadata.packages | Where-Object { $_.name -eq "ghitabrowser" } | Select-Object -First 1
if (-not $package) {
    throw "Unable to read the GhitaBrowser package version."
}

$version = $package.version
$sourceIcon = Join-Path $projectRoot "icon.ico"
$destination = Join-Path $env:LOCALAPPDATA "Programs\GhitaBrowser"
$releaseDirectory = Join-Path $projectRoot "target\release"
$binaries = [ordered]@{
    "ghitabrowser.exe" = "GhitaBrowser.exe"
    "ghita-renderer-worker.exe" = "ghita-renderer-worker.exe"
    "ghita-browser-child.exe" = "ghita-browser-child.exe"
}

foreach ($sourceName in $binaries.Keys) {
    $source = Join-Path $releaseDirectory $sourceName
    if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
        throw "Release build file not found: $source`nRun 'cargo build --release --locked -j 1' first."
    }
    $productVersion = (Get-Item -LiteralPath $source).VersionInfo.ProductVersion
    if (-not ([string]$productVersion).StartsWith($version, [StringComparison]::Ordinal)) {
        throw "Binary '$source' has ProductVersion '$productVersion', expected '$version'."
    }
}

New-Item -ItemType Directory -Force -Path $destination | Out-Null
foreach ($sourceName in $binaries.Keys) {
    Copy-Item -LiteralPath (Join-Path $releaseDirectory $sourceName) -Destination (Join-Path $destination $binaries[$sourceName]) -Force
}
Copy-Item -LiteralPath $sourceIcon -Destination (Join-Path $destination "icon.ico") -Force

$installedExe = Join-Path $destination "GhitaBrowser.exe"
$desktop = [Environment]::GetFolderPath("Desktop")
$shortcutPath = Join-Path $desktop "GhitaBrowser.lnk"
$shell = New-Object -ComObject WScript.Shell
$shortcut = $shell.CreateShortcut($shortcutPath)
$shortcut.TargetPath = $installedExe
$shortcut.WorkingDirectory = $destination
$shortcut.Description = "GhitaBrowser v$version"
$shortcut.IconLocation = "$(Join-Path $destination 'icon.ico'),0"
$shortcut.Save()

Write-Host "Installed GhitaBrowser v$version to $installedExe"
Write-Host "Shortcut: $shortcutPath"
