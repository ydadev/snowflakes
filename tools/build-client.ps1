[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$sfRoot = Split-Path -Parent $PSScriptRoot
$sfPreviousFlags = $env:CARGO_ENCODED_RUSTFLAGS
$sfPreviousCargo = $env:CARGO_HOME
$sfPreviousRustup = $env:RUSTUP_HOME
$sfPreviousPath = $env:PATH
Import-Module (Join-Path $PSScriptRoot 'BuildPaths.psm1') -Force
Push-Location -LiteralPath $sfRoot
try {
    $sfPortableCargo = Join-Path $sfRoot '.local/tools/cargo'
    if (Test-Path -LiteralPath (Join-Path $sfPortableCargo 'bin/cargo.exe')) {
        $env:CARGO_HOME = $sfPortableCargo
        $env:RUSTUP_HOME = Join-Path $sfRoot '.local/tools/rustup'
        $env:PATH = (Join-Path $sfPortableCargo 'bin') + [IO.Path]::PathSeparator + $env:PATH
    }
    $env:CARGO_ENCODED_RUSTFLAGS = Get-SnowRustFlags -Root $sfRoot
    & cargo build --workspace --release --locked
    if ($LASTEXITCODE -ne 0) { throw 'Client release build failed' }
    & (Join-Path $PSScriptRoot 'check-client-artifact.ps1')
} finally {
    $env:CARGO_ENCODED_RUSTFLAGS = $sfPreviousFlags
    $env:CARGO_HOME = $sfPreviousCargo
    $env:RUSTUP_HOME = $sfPreviousRustup
    $env:PATH = $sfPreviousPath
    Pop-Location
}
