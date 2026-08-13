param(
    [Parameter(Mandatory = $true)]
    [string]$InstallerPath,

    [Parameter(Mandatory = $true)]
    [string]$VMName,

    [Parameter(Mandatory = $true)]
    [PSCredential]$Credential,

    [string]$ExpectedPublisher = "GhitaBrowser",

    [string]$ReportPath = "phase18-clean-vm-report.json"
)

$ErrorActionPreference = "Stop"
$installer = [IO.Path]::GetFullPath($InstallerPath)
if (-not (Test-Path -LiteralPath $installer -PathType Leaf)) {
    throw "Installer not found: $installer"
}
$signature = Get-AuthenticodeSignature -LiteralPath $installer
if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
    throw "Phase 18 requires a valid Authenticode signature; status was $($signature.Status)."
}
if (-not $signature.SignerCertificate.Subject.Contains($ExpectedPublisher, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Installer signer '$($signature.SignerCertificate.Subject)' does not contain '$ExpectedPublisher'."
}

$session = New-PSSession -VMName $VMName -Credential $Credential
try {
    $remoteRoot = "C:\GhitaBrowser-Phase18-Gate"
    Invoke-Command -Session $session -ScriptBlock {
        param($root)
        if (Test-Path -LiteralPath $root) {
            Remove-Item -LiteralPath $root -Recurse -Force
        }
        New-Item -ItemType Directory -Path $root | Out-Null
    } -ArgumentList $remoteRoot
    $remoteInstaller = Join-Path $remoteRoot "GhitaBrowser-Setup.exe"
    Copy-Item -LiteralPath $installer -Destination $remoteInstaller -ToSession $session

    $result = Invoke-Command -Session $session -ScriptBlock {
        param($setup, $root)
        $ErrorActionPreference = "Stop"
        $installDir = Join-Path $env:LOCALAPPDATA "Programs\GhitaBrowser"
        $registryTargets = @(
            "HKCU:\Software\GhitaBrowser",
            "HKCU:\Software\Classes\GhitaBrowser.Document"
        )
        if ((Test-Path -LiteralPath $installDir) -or ($registryTargets | Where-Object { Test-Path $_ })) {
            throw "VM is not clean: an existing GhitaBrowser installation or registry key was found."
        }

        $install = Start-Process -FilePath $setup -ArgumentList "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/SP-" -Wait -PassThru
        if ($install.ExitCode -ne 0) {
            throw "Installer exited with $($install.ExitCode)."
        }
        $browser = Join-Path $installDir "GhitaBrowser.exe"
        $worker = Join-Path $installDir "ghita-renderer-worker.exe"
        if (-not (Test-Path -LiteralPath $browser -PathType Leaf) -or -not (Test-Path -LiteralPath $worker -PathType Leaf)) {
            throw "Installed browser/worker files are incomplete."
        }
        $version = (Get-Item -LiteralPath $browser).VersionInfo.ProductVersion
        if (-not $version.StartsWith("2.0.0", [StringComparison]::Ordinal)) {
            throw "Installed binary version is '$version', expected 2.0.0."
        }

        $smokeReport = Join-Path $root "installed-smoke.json"
        $smoke = Start-Process -FilePath $browser -ArgumentList "--release-smoke-report=$smokeReport" -Wait -PassThru
        if ($smoke.ExitCode -ne 0 -or -not (Test-Path -LiteralPath $smokeReport -PathType Leaf)) {
            throw "Installed headless smoke gate failed with exit code $($smoke.ExitCode)."
        }
        $smokeData = Get-Content -LiteralPath $smokeReport -Raw | ConvertFrom-Json
        if (-not $smokeData.passed -or -not $smokeData.worker -or -not $smokeData.runtime -or -not $smokeData.scene) {
            throw "Installed smoke report did not pass every subsystem."
        }

        $uninstaller = Join-Path $installDir "unins000.exe"
        if (-not (Test-Path -LiteralPath $uninstaller -PathType Leaf)) {
            throw "Uninstaller is missing."
        }
        $uninstall = Start-Process -FilePath $uninstaller -ArgumentList "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART" -Wait -PassThru
        if ($uninstall.ExitCode -ne 0) {
            throw "Uninstaller exited with $($uninstall.ExitCode)."
        }

        $residue = @()
        if (Test-Path -LiteralPath $installDir) { $residue += $installDir }
        foreach ($target in $registryTargets) {
            if (Test-Path $target) { $residue += $target }
        }
        $openWithTargets = @(
            "HKCU:\Software\Classes\.html\OpenWithProgids",
            "HKCU:\Software\Classes\.htm\OpenWithProgids",
            "HKCU:\Software\Classes\.xhtml\OpenWithProgids",
            "HKCU:\Software\Classes\.pdf\OpenWithProgids"
        )
        foreach ($target in $openWithTargets) {
            if ((Get-ItemProperty -Path $target -Name "GhitaBrowser.Document" -ErrorAction SilentlyContinue)."GhitaBrowser.Document" -ne $null) {
                $residue += "$target::GhitaBrowser.Document"
            }
        }
        $shortcutTargets = @(
            (Join-Path ([Environment]::GetFolderPath("Desktop")) "GhitaBrowser.lnk"),
            (Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\GhitaBrowser")
        )
        foreach ($target in $shortcutTargets) {
            if (Test-Path -LiteralPath $target) { $residue += $target }
        }
        if ($residue.Count -ne 0) {
            throw "Uninstall left residue: $($residue -join '; ')"
        }
        [PSCustomObject]@{
            passed = $true
            phase = 18
            version = $version
            installedSmoke = $smokeData
            uninstallResidue = @()
            vm = $env:COMPUTERNAME
            completedUtc = [DateTime]::UtcNow.ToString("o")
        }
    } -ArgumentList $remoteInstaller, $remoteRoot

    $resolvedReport = [IO.Path]::GetFullPath($ReportPath)
    $result | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $resolvedReport -Encoding utf8
    Write-Host "Phase 18 clean-VM gate passed. Report: $resolvedReport"
} finally {
    if ($session) {
        Remove-PSSession $session
    }
}
