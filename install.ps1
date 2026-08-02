# GhitaBrowser v0.6.1 installer: copies fresh build, updates desktop shortcut
$ErrorActionPreference = "Stop"

$src  = "E:\GhitaBrowser\target\release\ghitabrowser.exe"
$dest = "$env:LOCALAPPDATA\Programs\GhitaBrowser"

# 1. Install directory + copy the new exe
if (-not (Test-Path $src)) {
    throw "Release build not found: $src`nRun 'cargo build --release' first."
}
New-Item -ItemType Directory -Force -Path $dest | Out-Null
Copy-Item -Force $src (Join-Path $dest "GhitaBrowser.exe")
Copy-Item -Force "E:\GhitaBrowser\icon.ico" (Join-Path $dest "icon.ico")

$exe = Join-Path $dest "GhitaBrowser.exe"

# 2. Old shortcut on desktop -> point to new exe, icon from embedded resource
$desktop = [Environment]::GetFolderPath("Desktop")
$lnk = Join-Path $desktop "GhitaBrowser.lnk"
$ws = New-Object -ComObject WScript.Shell
if (Test-Path $lnk) { Remove-Item $lnk -Force }
$sc = $ws.CreateShortcut($lnk)
$sc.TargetPath = $exe
$sc.WorkingDirectory = $dest
$sc.Description = "GhitaBrowser v0.6.1"
$sc.IconLocation = "$exe,0"
$sc.Save()

Write-Host "INSTALLED: $exe"
Write-Host "SHORTCUT:  $lnk"
