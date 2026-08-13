# Local GhitaBrowser developer installer.
$ErrorActionPreference = "Stop"

$projectRoot = $PSScriptRoot
$metadata = cargo metadata --no-deps --format-version 1 --manifest-path (Join-Path $projectRoot "Cargo.toml") | ConvertFrom-Json
$package = $metadata.packages | Where-Object { $_.name -eq "ghitabrowser" } | Select-Object -First 1
if (-not $package) {
    throw "Unable to read the GhitaBrowser package version."
}

$version = $package.version
$sourceExe = Join-Path $projectRoot "target\release\ghitabrowser.exe"
$sourceIcon = Join-Path $projectRoot "icon.ico"
$destination = Join-Path $env:LOCALAPPDATA "Programs\GhitaBrowser"

if (-not (Test-Path -LiteralPath $sourceExe -PathType Leaf)) {
    throw "Release build not found: $sourceExe`nRun 'cargo build --release --locked' first."
}

New-Item -ItemType Directory -Force -Path $destination | Out-Null
Copy-Item -LiteralPath $sourceExe -Destination (Join-Path $destination "GhitaBrowser.exe") -Force
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
