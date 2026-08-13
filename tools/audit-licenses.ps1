$ErrorActionPreference = 'Stop'

$metadata = cargo metadata --format-version 1 --locked | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) {
    throw 'cargo metadata failed'
}

$workspaceMembers = [System.Collections.Generic.HashSet[string]]::new()
foreach ($member in $metadata.workspace_members) {
    [void]$workspaceMembers.Add($member)
}

$missing = @($metadata.packages | Where-Object {
    -not $workspaceMembers.Contains($_.id) -and
    [string]::IsNullOrWhiteSpace($_.license)
})
if ($missing.Count -gt 0) {
    $names = $missing | ForEach-Object { "$($_.name) $($_.version)" }
    throw "Dependencies without license metadata: $($names -join ', ')"
}

$copyleftOnly = @($metadata.packages | Where-Object {
    if ($workspaceMembers.Contains($_.id)) { return $false }
    $expression = $_.license
    $hasCopyleft = $expression -match '(?i)(?:^|[^A-Z])(?:A?GPL|LGPL)-'
    $hasPermissiveAlternative = $expression -match '(?i)\bOR\b' -and
        $expression -match '(?i)(?:MIT|Apache|BSD|ISC|Zlib|0BSD|Unlicense)'
    $hasCopyleft -and -not $hasPermissiveAlternative
})
if ($copyleftOnly.Count -gt 0) {
    $names = $copyleftOnly | ForEach-Object {
        "$($_.name) $($_.version) [$($_.license)]"
    }
    throw "Copyleft-only dependencies require explicit legal approval: $($names -join ', ')"
}

"License metadata audit passed for $($metadata.packages.Count) locked packages."
