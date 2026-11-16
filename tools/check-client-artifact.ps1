[CmdletBinding()]
param([string]$Binary = '')

$ErrorActionPreference = 'Stop'
$sfRoot = Split-Path -Parent $PSScriptRoot
if (-not $Binary) { $Binary = Join-Path $sfRoot 'target/release/snowflakes.exe' }
$sfBytes = [IO.File]::ReadAllBytes([IO.Path]::GetFullPath($Binary))
if ($sfBytes.Length -lt 64 -or [BitConverter]::ToUInt16($sfBytes, 0) -ne 0x5a4d) {
    throw 'Client artifact is not a PE image'
}
$sfPe = [BitConverter]::ToInt32($sfBytes, 0x3c)
if ($sfPe -lt 64 -or $sfPe -gt $sfBytes.Length - 96 -or
    [BitConverter]::ToUInt32($sfBytes, $sfPe) -ne 0x4550 -or
    [BitConverter]::ToUInt16($sfBytes, $sfPe + 4) -ne 0x8664 -or
    [BitConverter]::ToUInt16($sfBytes, $sfPe + 24) -ne 0x20b -or
    [BitConverter]::ToUInt16($sfBytes, $sfPe + 24 + 68) -ne 2) {
    throw 'Client artifact must be an x64 Windows GUI PE image'
}
$sfViews = @(
    [Text.Encoding]::UTF8.GetString($sfBytes),
    [Text.Encoding]::Unicode.GetString($sfBytes),
    [Text.Encoding]::Unicode.GetString($sfBytes, 1, $sfBytes.Length - 1)
)
$sfRoots = @($sfRoot, $env:CARGO_HOME, $env:RUSTUP_HOME, $env:USERPROFILE) |
    Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
foreach ($sfPath in $sfRoots) {
    $sfFull = [IO.Path]::GetFullPath($sfPath).TrimEnd([char[]]'\/')
    foreach ($sfVariant in @($sfFull, $sfFull.Replace('\', '/')) | Select-Object -Unique) {
        foreach ($sfView in $sfViews) {
            if ($sfView.IndexOf($sfVariant, [StringComparison]::OrdinalIgnoreCase) -ge 0) {
                # Never print the matched value or an excerpt from the binary.
                throw 'Client artifact contains an unremapped build-root path'
            }
        }
    }
}
Write-Output 'Client artifact: neutral build paths and x64 GUI passed'
